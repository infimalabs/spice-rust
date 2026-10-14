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
                let quotes = present_with(
                    store,
                    &source.text,
                    depth.checked_add(1).unwrap_or(QUOTE_DEPTH_LIMIT),
                );
                let inlined = wire::media::with_media_payloads(&source.text, &source.media)
                    .map_err(|_| QuoteUnavailable::Unreadable)?;
                let inlined = match inlined {
                    std::borrow::Cow::Owned(text) => Some(text),
                    std::borrow::Cow::Borrowed(_) => None,
                };
                Ok(QuotePresentation::Resolved {
                    target: quote.target.clone(),
                    text: inlined.unwrap_or(source.text),
                    author: source.author,
                    disposition: source.disposition,
                    quotes,
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
        let authored = |text| SourceQuote {
            identity: None,
            text,
            author: QuoteAuthor::Directive {
                actor: envelope.actor.clone(),
            },
            disposition: None,
            media: Vec::new(),
        };
        if let Some(posted) = envelope.fact.read::<wire::rooms::RoomPosted>() {
            return Ok(authored(posted.text));
        }
        if let Some(published) = envelope.fact.read::<wire::rooms::RoomPublished>()
            && let wire::rooms::RoomPublicationContent::Authored { text } = published.content
        {
            return Ok(authored(text));
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Default)]
    struct MemoryStore {
        tape: BTreeMap<String, SourceQuote>,
        steering: BTreeMap<String, SourceQuote>,
        room: BTreeMap<String, SourceQuote>,
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

        fn room(
            &self,
            room: &wire::TaskStamp,
            publication: &wire::rooms::RoomAuthorityStamp,
        ) -> Result<SourceQuote, SourceError> {
            if self.unreadable {
                return Err(SourceError::Unreadable);
            }
            self.room
                .get(&format!("{room}/{publication}"))
                .cloned()
                .ok_or(SourceError::Unknown)
        }
    }

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")
            .parse()
            .expect("canonical UUID")
    }

    const PREDECESSOR_TEXT: &str = "answer from predecessor";
    const SUCCESSOR_TEXT: &str = "answer from successor";
    const INTENT_TEXT: &str = "ACK 1kQwPZtK: intent-backed reply";
    const INTENT_RECORD: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-1","name":"read","intent":"ACK 1kQwPZtK: intent-backed reply","arguments":{}}],"stopReason":"toolUse"}}"#;

    struct DurableQuoteFixture {
        root: PathBuf,
        tape_root: PathBuf,
        tape: tape::Tape,
        bus: bus::Bus,
        lane: CanonicalUuid,
        source: CanonicalUuid,
        predecessor_locus: tape::Locus,
        successor_locus: tape::Locus,
        intent_locus: tape::Locus,
        end_offset: u64,
    }

    impl DurableQuoteFixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-quote-source-session-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let lane = uuid(1);
            let vendor: wire::Name = "omp".parse().expect("fixture vendor");
            let tape_root = root.join("tape");
            let tape_path = tape_root.join(lane.to_string()).join(vendor.as_str());
            fs::create_dir_all(&tape_path).expect("fixture tape directory exists");
            let tape = tape::Tape::open(&tape_path).expect("fixture tape opens");
            for text in [PREDECESSOR_TEXT, SUCCESSOR_TEXT] {
                tape.tee(
                    format!(
                        "{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}],\"stopReason\":\"stop\"}}}}\n"
                    )
                    .as_bytes(),
                )
                .expect("fixture prose is durable");
            }
            tape.tee(format!("{INTENT_RECORD}\n").as_bytes())
                .expect("fixture tool intent is durable");
            let scan =
                listen::scan(&tape_path, listen::Dialect::Omp, None).expect("fixture tape scans");
            let predecessor_locus = scan.records[0].locus;
            let successor_locus = scan.records[1].locus;
            let intent_locus = scan.records[2].locus;
            let source = scan
                .checkpoint
                .source
                .as_str()
                .parse::<CanonicalUuid>()
                .expect("fixture tape source is canonical");
            let bus = session_bus(
                lane,
                source,
                vendor,
                predecessor_locus,
                successor_locus,
                scan.checkpoint.next,
            );
            Self {
                root,
                tape_root,
                tape,
                bus,
                lane,
                source,
                predecessor_locus,
                successor_locus,
                intent_locus,
                end_offset: scan.checkpoint.next.offset,
            }
        }

        fn target(&self, source: CanonicalUuid, locus: tape::Locus, text: &str) -> QuoteTarget {
            QuoteTarget::Tape {
                lane: self.lane,
                source,
                locus: wire::MessageLocus {
                    member: locus.member,
                    offset: locus.offset,
                    length: locus.length,
                    event: 0,
                },
                digest: QuoteTarget::text_digest(text),
            }
        }

        fn finish(self) {
            self.bus.shutdown().expect("fixture bus stops");
            drop(self.tape);
            fs::remove_dir_all(self.root).expect("fixture cleanup succeeds");
        }
    }

    fn session_bus(
        lane: CanonicalUuid,
        source: CanonicalUuid,
        vendor: wire::Name,
        predecessor: tape::Locus,
        successor: tape::Locus,
        end: listen::Position,
    ) -> bus::Bus {
        let bus = bus::Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        let mut write = journal::JournalWrite::new(
            wire::Actor::Spice,
            format!("lane:{lane}").parse().expect("fixture origin"),
            wire::Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
            wire::Producer::ResidentRecognition,
        );
        for (occupant, worktree, identity, locus) in [
            (uuid(3), "/worktrees/predecessor", "session-a", predecessor),
            (uuid(4), "/worktrees/successor", "session-b", successor),
        ] {
            write
                .fact(
                    wire::body::LaneSessionDeclared {
                        lane,
                        occupant,
                        worktree: worktree.to_owned(),
                        vendor: vendor.clone(),
                        launch: wire::body::LaneLaunchIdentity::new("sonnet", "high"),
                        epoch: uuid(5),
                        stream: "session".to_owned(),
                        source,
                        identity: identity.to_owned(),
                        member: locus.member,
                        offset: locus.offset,
                        length: locus.length,
                    },
                    journal::FactCause::Root,
                )
                .expect("fixture session declaration builds");
        }
        let next = wire::body::TranscriptPosition {
            member: end.member,
            offset: end.offset,
        };
        let ingested = write
            .fact(
                wire::body::JournalIngested {
                    lane,
                    vendor: vendor.clone(),
                    stream: "session".to_owned(),
                    source,
                    supersedes: None,
                    from: wire::body::TranscriptPosition {
                        member: predecessor.member,
                        offset: predecessor.offset,
                    },
                    to: next,
                    usage: Vec::new(),
                    non_usage_records: 3,
                    residue_records: 0,
                },
                journal::FactCause::Root,
            )
            .expect("fixture ingestion builds");
        write.transcript_source_head(ingested, move |_| wire::TranscriptSourceHead {
            lane,
            vendor,
            stream: "session".to_owned(),
            source,
            next,
        });
        assert!(matches!(
            bus.commit(write).expect("fixture facts commit"),
            bus::CommitOutcome::Committed(_)
        ));
        bus
    }

    #[test]
    fn durable_source_loci_resolve_each_session_author_after_lane_renewal() {
        let fixture = DurableQuoteFixture::new();
        let reader = fixture.bus.reader();
        let resolver = Resolver::from_tape(&fixture.tape_root, &reader);
        for (locus, text, occupant) in [
            (fixture.predecessor_locus, PREDECESSOR_TEXT, uuid(3)),
            (fixture.successor_locus, SUCCESSOR_TEXT, uuid(4)),
            (fixture.intent_locus, INTENT_TEXT, uuid(4)),
        ] {
            assert_eq!(
                resolver.resolve(&fixture.target(fixture.source, locus, text).to_string(), 0),
                QuoteResolution::Resolved(ResolvedQuote {
                    text: text.to_owned(),
                    author: QuoteAuthor::Agent {
                        lane: fixture.lane,
                        occupant,
                    },
                    disposition: None,
                })
            );
        }
        for unavailable in [
            fixture.target(uuid(9), fixture.predecessor_locus, PREDECESSOR_TEXT),
            fixture.target(
                fixture.source,
                tape::Locus::new(fixture.successor_locus.member, fixture.end_offset + 1, 1),
                SUCCESSOR_TEXT,
            ),
        ] {
            assert!(matches!(
                resolver.resolve(&unavailable.to_string(), 0),
                QuoteResolution::Unavailable {
                    reason: QuoteUnavailable::Unknown,
                    ..
                }
            ));
        }
        drop(resolver);
        drop(reader);
        fixture.finish();
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
    fn every_authority_kind_resolves_with_recorded_authorship() {
        let mut store = MemoryStore::default();
        let tape = tape_target("agent text");
        store.tape.insert(
            format!("{}/{}/3/5/8/13", uuid(1), uuid(2)),
            SourceQuote {
                identity: None,
                text: "agent text".to_owned(),
                author: QuoteAuthor::Agent {
                    lane: uuid(1),
                    occupant: uuid(3),
                },
                disposition: None,
                media: Vec::new(),
            },
        );
        let steering = steering_target("dTVLR0qL0", "operator text");
        store.steering.insert(
            "dTVLR0qL0".to_owned(),
            SourceQuote {
                identity: None,
                text: "operator text".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Acked),
                media: Vec::new(),
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

        let room: wire::TaskStamp = "dVrD59Y8N".parse().expect("fixture room");
        let publication: wire::rooms::RoomAuthorityStamp =
            "dVsYdDj7d".parse().expect("fixture publication");
        store.room.insert(
            format!("{room}/{publication}"),
            SourceQuote {
                identity: None,
                text: "room post".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: None,
                media: Vec::new(),
            },
        );
        let target = |text| QuoteTarget::Room {
            room: room.clone(),
            publication: publication.clone(),
            digest: QuoteTarget::text_digest(text),
        };
        assert_eq!(
            resolve_with(&store, &target("room post").to_string(), 0),
            QuoteResolution::Resolved(ResolvedQuote {
                text: "room post".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: None,
            })
        );
        let changed = target("other words").to_string();
        assert_eq!(
            resolve_with(&store, &changed, 0),
            unavailable(&changed, QuoteUnavailable::DigestMismatch)
        );
    }

    #[test]
    fn agent_quote_presentation_carries_authenticated_directive_context() {
        let mut store = MemoryStore::default();
        let authored = "ACK dTVLR0qL0: completed";
        let operator = "inspect the quote chain";
        let directive = steering_target("dTVLR0qL0", operator);
        let presented = format!("{}\n\ncompleted", directive.markdown());
        let tape = tape_target(authored);
        store.tape.insert(
            format!("{}/{}/3/5/8/13", uuid(1), uuid(2)),
            SourceQuote {
                identity: Some(authored.to_owned()),
                text: presented.clone(),
                author: QuoteAuthor::Agent {
                    lane: uuid(1),
                    occupant: uuid(3),
                },
                disposition: None,
                media: Vec::new(),
            },
        );
        store.steering.insert(
            "dTVLR0qL0".to_owned(),
            SourceQuote {
                identity: None,
                text: operator.to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Acked),
                media: Vec::new(),
            },
        );

        let source = tape.markdown();
        assert_eq!(
            expand_with(&store, &source, 0),
            format!("{operator}\n\ncompleted")
        );
        let presentation = present_with(&store, &source, 0);
        let [QuotePresentation::Resolved { text, quotes, .. }] = presentation.as_slice() else {
            panic!("agent quote resolves");
        };
        assert_eq!(text, &presented);
        assert!(matches!(
            quotes.as_slice(),
            [QuotePresentation::Resolved {
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Acked),
                ..
            }]
        ));
    }

    #[test]
    fn nested_quote_presentation_inlines_each_sources_own_media() {
        let image = wire::Media::from_base64("image.png", "image/png", "iVBORw0KGgo=")
            .expect("fixture media");
        let inner_text = format!("look\n\n{}", image.reference_markdown().expect("reference"));
        let inner = steering_target("dTVLR0qL0", &inner_text);
        let outer_text = format!("see {}", inner.markdown());
        let outer = steering_target("dTVLR0qL1", &outer_text);
        let mut store = MemoryStore::default();
        for (key, text, media) in [
            ("dTVLR0qL0", &inner_text, vec![image.clone()]),
            ("dTVLR0qL1", &outer_text, Vec::new()),
        ] {
            store.steering.insert(
                key.to_owned(),
                SourceQuote {
                    identity: None,
                    text: text.clone(),
                    author: QuoteAuthor::Directive {
                        actor: wire::Actor::Operator,
                    },
                    disposition: Some(wire::Disposition::Acked),
                    media,
                },
            );
        }
        let message = outer.markdown();

        let presented = present_with(&store, &message, 0);
        let [QuotePresentation::Resolved { text, quotes, .. }] = presented.as_slice() else {
            panic!("outer quote resolves");
        };
        assert_eq!(text, &outer_text);
        let [QuotePresentation::Resolved { text, quotes, .. }] = quotes.as_slice() else {
            panic!("nested quote resolves against its raw digest");
        };
        assert_eq!(
            text,
            &format!("look\n\n{}", image.markdown().expect("markdown"))
        );
        assert!(quotes.is_empty());
        assert_eq!(
            expand_with(&store, &message, 0),
            format!("see {inner_text}")
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
                identity: None,
                text: "forged".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Live),
                media: Vec::new(),
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
                    identity: None,
                    text: returned_text,
                    author: QuoteAuthor::Directive {
                        actor: wire::Actor::Operator,
                    },
                    disposition: Some(if index == 4 {
                        wire::Disposition::Acked
                    } else {
                        wire::Disposition::Live
                    }),
                    media: Vec::new(),
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
                identity: None,
                text: text.to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Live),
                media: Vec::new(),
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

    #[test]
    fn delivery_expands_nested_quotes_and_names_unavailable_sources() {
        let mut store = MemoryStore::default();
        let inner_text = "the authenticated inner words";
        let inner = steering_target("dTVLR0qL0", inner_text);
        store.steering.insert(
            "dTVLR0qL0".to_owned(),
            SourceQuote {
                identity: None,
                text: inner_text.to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Live),
                media: Vec::new(),
            },
        );
        let outer_text = format!("before {} after", inner.markdown());
        let outer = steering_target("dTVLR0qL1", &outer_text);
        store.steering.insert(
            "dTVLR0qL1".to_owned(),
            SourceQuote {
                identity: None,
                text: outer_text,
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Live),
                media: Vec::new(),
            },
        );
        let missing = steering_target("dTVLR0qL2", "missing");
        let source = format!("say {} then {}", outer.markdown(), missing.markdown());

        assert_eq!(
            expand_with(&store, &source, 0),
            format!(
                "say before {inner_text} after then \
                 [quote unavailable: reason=unknown source={:?}]",
                missing.to_string(),
            )
        );
    }

    #[test]
    fn delivery_depth_limit_leaves_one_explicit_source_stub() {
        let mut store = MemoryStore::default();
        let mut returned_text = "terminal text".to_owned();
        let mut outer = None;
        for index in 0..9 {
            let key = format!("0000000{index}");
            let target = steering_target(&key, &returned_text);
            store.steering.insert(
                key,
                SourceQuote {
                    identity: None,
                    text: returned_text,
                    author: QuoteAuthor::Directive {
                        actor: wire::Actor::Operator,
                    },
                    disposition: Some(wire::Disposition::Live),
                    media: Vec::new(),
                },
            );
            returned_text = target.markdown();
            outer = Some(target);
        }

        let expanded = expand_with(
            &store,
            &outer.expect("chain has an outer target").markdown(),
            0,
        );
        assert!(!expanded.contains("[quote]("), "{expanded}");
        assert!(expanded.contains("[quote unavailable: reason=depth-exceeded source="));
        assert_eq!(expanded.lines().count(), 1);
    }

    /// One settled omp assistant message whose visible prose the provider
    /// folded into reasoning, so its only verbatim words are the call intent.
    const NARRATED_ASK: &str = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"ASK #ROOMS-dVrD255Kf: paraphrased question"},{"type":"toolCall","id":"call-1","name":"shell","arguments":{"command":"true","i":"ASK #ROOMS-dVrD255Kf: arguments are not authored words"},"intent":"ASK #ROOMS-dVrD255Kf: Which gesture activates a design?"}],"stopReason":"toolUse"}}"#;

    fn narrated_events() -> Vec<listen::Event> {
        let decoded = listen::decode(
            listen::Dialect::Omp,
            NARRATED_ASK.as_bytes(),
            tape::Locus::new(1, 0, NARRATED_ASK.len() as u64),
        )
        .expect("fixture record decodes");
        decoded.events().to_vec()
    }

    fn range(start: u64, end: u64) -> wire::rooms::TextRange {
        wire::rooms::TextRange { start, end }
    }

    #[test]
    fn a_packet_cited_from_a_call_intent_resolves_to_its_exact_bytes() {
        let events = narrated_events();
        let call = events
            .iter()
            .position(|event| matches!(event, listen::Event::Tool(_)))
            .expect("the record carries the call");
        let header = "ASK #ROOMS-dVrD255Kf: ".len() as u64;
        let end = "ASK #ROOMS-dVrD255Kf: Which gesture activates a design?".len() as u64;

        assert_eq!(
            hunk_text(&events, call as u32, range(header, end)),
            Ok("Which gesture activates a design?".to_owned())
        );
    }

    #[test]
    fn a_hunk_naming_reasoning_or_an_out_of_range_span_reads_as_changed() {
        let events = narrated_events();
        let reasoning = events
            .iter()
            .position(|event| matches!(event, listen::Event::Reasoning(_)))
            .expect("the record carries reasoning");
        let call = events
            .iter()
            .position(|event| matches!(event, listen::Event::Tool(_)))
            .expect("the record carries the call");
        let changed = Err(wire::rooms::RoomSourceUnavailable::Changed {});

        assert_eq!(hunk_text(&events, reasoning as u32, range(0, 3)), changed);
        assert_eq!(hunk_text(&events, call as u32, range(14, 999)), changed);
        assert_eq!(hunk_text(&events, u32::MAX, range(0, 3)), changed);
    }

    #[test]
    fn operator_prose_is_never_packet_text() {
        let operator = listen::Event::StreamedProse(listen::Prose {
            role: listen::ProseRole::Operator,
            text: "ASK #ROOMS-dVrD255Kf: not authored by the lane".to_owned(),
            phase: None,
            media: Vec::new(),
            external_media: Vec::new(),
            locus: tape::Locus::new(1, 0, 1),
        });

        assert_eq!(packet_text(&operator), None);
    }
}
