//! `spice room history HANDLE`: one anchor task's room, oldest publication first.
//!
//! The handle resolves by birth stamp, so every spelling a re-homed task has
//! carried reads the same room, and the page names the current one. Each
//! publication is projected from the authority that owns its bytes: operator
//! text from its journal fact, an agent packet from its author's durable
//! tape, and a directed route's receipt from its directive's steering head.
//! A conversation publication (MSG, ASK, or ANS) also names its operator
//! item receipt and its question result at their current heads, apart from
//! what the room records; [`inbox`] and [`result`] read the same projections.
//! A source that no longer yields the bytes renders as unavailable with its
//! reason. Publications page whole in journal order through [`Page`]; a page
//! that withholds any continues at the first withheld publication. Reading
//! writes nothing.

mod conversation;
pub(crate) mod inbox;
pub(crate) mod relevance;
pub(crate) mod result;

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use clap::ArgMatches;
use serde::Serialize;
use spice::repository::Repository;
use wire::output::{Bounded, Export};
use wire::rooms::{
    RoomAuthorityStamp, RoomCoordinationStarted, RoomMessaged, RoomPosted, RoomPublicationContent,
    RoomPublished, RoomPutAside, RoomSource, RoomSourceUnavailable, TranscriptHunk,
};
use wire::{
    Actor, CanonicalUuid, Disposition, Fault, FaultClass, Instant, Outcome, Refusal, Reply,
    ReplyPosition, Seq, SequenceCode, TaskHandle, TaskStamp,
};

use crate::read_page::{Page, Paged};

use conversation::ConversationView;

/// What a continuation's `--after` names.
const CURSOR: &str = "a publication of this room";

/// One page of a room, or the receipt of a page exported whole.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum RoomHistory {
    Shown {
        room: TaskHandle,
        anchor: TaskStamp,
        page: Bounded<Publication>,
    },
    Exported {
        room: TaskHandle,
        anchor: TaskStamp,
        export: Export,
    },
}

/// One publication as its owning authorities state it.
#[derive(Debug, Serialize)]
pub(crate) struct Publication {
    publication: RoomAuthorityStamp,
    seq: Seq,
    at: Instant,
    /// The operator, or the lane whose agent authored the packet.
    actor: Actor,
    /// Occupant whose transcript authored an agent packet.
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<CanonicalUuid>,
    content: Content,
    /// Each directed route the packet opened into this room, with its receipt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    directed: Vec<Route>,
    /// The conversation this publication belongs to: its operator receipt and
    /// question result at their current heads.
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation: Option<ConversationView>,
}

/// A publication's bytes, or why its owning source cannot supply them.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum Content {
    Text {
        text: String,
    },
    /// A journal fact or artifact the publication cites rather than quotes.
    Linked {
        source: RoomSource,
    },
    OopsGroomed {
        grooming: wire::rooms::OopsGrooming,
    },
    Unavailable {
        source: RoomSource,
        unavailable: RoomSourceUnavailable,
    },
}

/// One directed delivery and its receipt.
#[derive(Debug, Serialize)]
struct Route {
    directive: CanonicalUuid,
    receipt: Receipt,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum Receipt {
    /// The directive's current steering standing.
    Steering {
        key: String,
        lane: CanonicalUuid,
        disposition: Disposition,
    },
    /// The journal holds no steering head for the directive.
    Missing,
}

/// Read one bounded page of a room from the working repository.
pub(crate) fn run(matches: &ArgMatches, contract: &crate::cli::Contract) -> Outcome<RoomHistory> {
    match discover() {
        Ok(repository) => run_at(&repository, matches, contract),
        Err(failure) => failure.outcome(),
    }
}

/// The repository the working directory belongs to.
fn discover() -> Result<Repository, Failure> {
    std::env::current_dir()
        .map_err(|error| {
            fault(
                FaultClass::Environment,
                "room::working-directory-unavailable",
                error,
            )
        })
        .and_then(|working| {
            Repository::discover(&working).map_err(|error| {
                fault(
                    FaultClass::Environment,
                    spice::repository::ResolveError::REASON,
                    error,
                )
            })
        })
}

/// The admitted state and journal reader of `repository`.
fn open(
    repository: &Repository,
) -> Result<(spice::repository::StatePaths, journal::JournalReader), Failure> {
    let state = repository
        .admit_state()
        .map_err(|error| fault(FaultClass::Environment, "room::state-unavailable", error))?;
    let journal = journal::JournalReader::open(state.journal())
        .map_err(|error| fault(FaultClass::Store, "room::journal-unreadable", error))?;
    Ok((state, journal))
}

/// [`run`] against `repository`.
pub(crate) fn run_at(
    repository: &Repository,
    matches: &ArgMatches,
    contract: &crate::cli::Contract,
) -> Outcome<RoomHistory> {
    let opened = || -> Result<_, Failure> {
        let projects = spice::config::capture_repository_scoped(repository, &[], None)
            .map_err(|error| {
                fault(
                    FaultClass::Environment,
                    "room::configuration-invalid",
                    error,
                )
            })?
            .resolved()
            .root()
            .tasks()
            .project_policy();
        let (state, journal) = open(repository)?;
        Ok((projects, state, journal))
    };
    match opened() {
        Ok((projects, state, journal)) => {
            answer(&journal, state.tape(), &projects, matches, contract.path)
        }
        Err(failure) => failure.outcome(),
    }
}

/// One page of the room `matches` names, read from `journal` and the lane
/// tapes under `tape`.
pub(crate) fn answer(
    journal: &journal::JournalReader,
    tape: &Path,
    projects: &board::ProjectPolicy,
    matches: &ArgMatches,
    path: &[&str],
) -> Outcome<RoomHistory> {
    match history(journal, tape, projects, matches, path) {
        Ok((position, body)) => Outcome::Reply(Reply { position, body }),
        Err(failure) => failure.outcome(),
    }
}

fn history(
    journal: &journal::JournalReader,
    tape: &Path,
    projects: &board::ProjectPolicy,
    matches: &ArgMatches,
    path: &[&str],
) -> Result<(ReplyPosition, RoomHistory), Failure> {
    let page = Page::from_matches(matches, path)?;
    let named = matches
        .get_one::<String>("HANDLE")
        .expect("generated room parser requires HANDLE");
    let named = named
        .parse::<TaskHandle>()
        .map_err(|error| task_invalid(named, &error))?;
    let anchor = named
        .born()
        .map_err(|error| task_invalid(named.as_str(), &error))?;
    let after = page
        .after()
        .map(|after| {
            after
                .parse::<RoomAuthorityStamp>()
                .map_err(|_| page.cursor_refusal(CURSOR))
        })
        .transpose()?;
    let recorded = journal
        .read_with(|read| recorded(read, &anchor, after.as_ref(), &page, projects))
        .map_err(|error| match error {
            journal::ReadError::Journal(error) => store(error),
            journal::ReadError::Answer(failure) => failure,
        })?;
    if let Some(notice) = crate::board_command::rename_notice(&named, &recorded.room) {
        eprintln!("{notice}");
    }
    let rows = recorded
        .rows
        .into_iter()
        .map(|row| {
            let key = row.publication.to_string();
            (row.project(tape), key)
        })
        .collect();
    let body = match page.admit(rows, recorded.population)? {
        Paged::Shown(page) => RoomHistory::Shown {
            room: recorded.room,
            anchor,
            page,
        },
        Paged::Exported(export) => RoomHistory::Exported {
            room: recorded.room,
            anchor,
            export,
        },
    };
    Ok((ReplyPosition::Snapshot(recorded.cursor), body))
}

/// Project admitted room envelopes through the same source-backed rendering as
/// the room history read.
pub(crate) fn project_publications(
    bus: &bus::Bus,
    tape: &Path,
    envelopes: Vec<wire::Envelope<wire::AnyFact>>,
) -> Result<Vec<Publication>, String> {
    let rows = bus
        .read(move |read| {
            envelopes
                .into_iter()
                .map(|envelope| row(read, envelope).map_err(Failure::guard))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| error.to_string())?;
    Ok(rows.into_iter().map(|row| row.project(tape)).collect())
}

/// What one read snapshot of the journal holds of a room.
struct Recorded {
    cursor: Seq,
    /// The anchor task's current handle.
    room: TaskHandle,
    /// Publications at or after the caller's cursor.
    population: usize,
    /// The first of them, at most one page and the record it withholds.
    rows: Vec<Row>,
}

/// One publication as the journal records it, before its source is read.
struct Row {
    publication: RoomAuthorityStamp,
    seq: Seq,
    at: Instant,
    actor: Actor,
    body: Body,
    directed: Vec<Route>,
    conversation: Option<ConversationView>,
}

enum Body {
    Published(RoomPublicationContent),
    Messaged {
        hunk: TranscriptHunk,
        author: CanonicalUuid,
    },
    Linked(RoomSource),
}

fn recorded(
    read: &journal::JournalRead<'_>,
    anchor: &TaskStamp,
    after: Option<&RoomAuthorityStamp>,
    page: &Page,
    projects: &board::ProjectPolicy,
) -> Result<Recorded, Failure> {
    let room = current_handle(read, anchor, projects)?;
    let cursor = read.cursor().map_err(store)?.ok_or_else(|| {
        fault(
            FaultClass::Store,
            "room::cursor-missing",
            "the journal names a task but no read position",
        )
    })?;
    let start = match after {
        None => None,
        Some(publication) => {
            let seq = read
                .room_publication_seq(anchor, publication)
                .map_err(store)?
                .ok_or_else(|| Failure::Refusal(page.cursor_refusal(CURSOR)))?;
            Seq::new(seq.get() - 1)
        }
    };
    let population = read.room_publication_count(anchor, start).map_err(store)?;
    let retain = page.retain().min(population);
    let budget = journal::FactReadBudget::new(
        retain.clamp(1, journal::FactReadBudget::RETAINED.rows()),
        journal::FactReadBudget::RETAINED.decoded_bytes(),
    )
    .map_err(store)?;
    let mut rows = Vec::with_capacity(retain);
    let mut next = start;
    while rows.len() < retain {
        let fetched = read
            .room_publications(anchor, next, budget)
            .map_err(store)?;
        for envelope in fetched.rows.into_iter().take(retain - rows.len()) {
            rows.push(row(read, envelope)?);
        }
        match fetched.continuation {
            Some(seq) => next = Some(seq),
            None => break,
        }
    }
    Ok(Recorded {
        cursor,
        room,
        population,
        rows,
    })
}

/// The current handle of the room anchored at `anchor`, refusing an anchor
/// that names no task.
fn current_handle(
    read: &journal::JournalRead<'_>,
    anchor: &TaskStamp,
    projects: &board::ProjectPolicy,
) -> Result<TaskHandle, Failure> {
    let heads = read.task_heads_for(anchor).map_err(store)?.ok_or_else(|| {
        Failure::Refusal(crate::read_page::refusal(
            "room",
            "task-unknown",
            Some(anchor.to_string()),
            "read the board and pass an existing task handle",
        ))
    })?;
    Ok(board::Entry::new(
        heads.statement().body().clone(),
        heads.workflow().body().clone(),
        heads.claim().map(|head| head.body().clone()),
        heads.lease().map(|head| head.body().clone()),
        projects,
    )
    .map_err(|error| fault(FaultClass::Store, "room::task-head-invalid", error))?
    .handle()
    .clone())
}

fn row(
    read: &journal::JournalRead<'_>,
    envelope: wire::Envelope<wire::AnyFact>,
) -> Result<Row, Failure> {
    let (publication, body, directives, conversed) =
        if let Some(published) = envelope.fact.read::<RoomPublished>() {
            (
                published.publication,
                Body::Published(published.content),
                Vec::new(),
                None,
            )
        } else if let Some(messaged) = envelope.fact.read::<RoomMessaged>() {
            let conversed = messaged_conversation(read, envelope.seq, &messaged.hunk)?;
            (
                messaged.publication,
                Body::Messaged {
                    hunk: messaged.hunk,
                    author: messaged.author,
                },
                messaged.directed,
                conversed,
            )
        } else if let Some(posted) = envelope.fact.read::<RoomPosted>() {
            let conversed = read
                .room_conversation(&posted.publication)
                .map_err(store)?
                .map(|(_, publication)| publication);
            (
                posted.publication,
                Body::Published(RoomPublicationContent::Authored { text: posted.text }),
                posted.directed,
                conversed,
            )
        } else if let Some(started) = envelope.fact.read::<RoomCoordinationStarted>() {
            (
                started.publication,
                Body::Linked(RoomSource::Journal {
                    fact: SequenceCode::new(envelope.seq.get()),
                }),
                Vec::new(),
                None,
            )
        } else if let Some(shelved) = envelope.fact.read::<RoomPutAside>() {
            (
                shelved.publication,
                Body::Linked(RoomSource::Journal {
                    fact: SequenceCode::new(envelope.seq.get()),
                }),
                Vec::new(),
                None,
            )
        } else {
            return Err(fault(
                FaultClass::Store,
                "room::publication-invalid",
                format!(
                    "room publication {} is a {} fact",
                    envelope.seq,
                    envelope.fact.kind()
                ),
            ));
        };
    let directed = directives
        .into_iter()
        .map(|directive| {
            Ok(Route {
                directive,
                receipt: receipt(read, directive)?,
            })
        })
        .collect::<Result<_, Failure>>()?;
    let conversation = conversed
        .map(|publication| conversation::conversation(read, &publication))
        .transpose()?;
    Ok(Row {
        publication,
        seq: envelope.seq,
        at: envelope.at,
        actor: envelope.actor,
        body,
        directed,
        conversation,
    })
}

/// The conversation record a lane packet's room record shares its write
/// with: the first one after it, proved by owning the same transcript hunk.
/// A packet that conversed with no operator or question has none.
fn messaged_conversation(
    read: &journal::JournalRead<'_>,
    seq: Seq,
    hunk: &TranscriptHunk,
) -> Result<Option<wire::rooms::RoomPublication>, Failure> {
    Ok(read
        .room_conversation_after(seq)
        .map_err(store)?
        .map(|(_, publication)| publication)
        .filter(|publication| {
            matches!(publication.source(), RoomSource::Transcript(owned) if owned == hunk)
        }))
}

/// A directive's current steering standing.
fn receipt(read: &journal::JournalRead<'_>, directive: CanonicalUuid) -> Result<Receipt, Failure> {
    Ok(read
        .steering_head(directive)
        .map_err(store)?
        .map_or(Receipt::Missing, |head| {
            let head = head.into_body();
            Receipt::Steering {
                key: head.key,
                lane: head.lane,
                disposition: head.disposition,
            }
        }))
}

impl Row {
    /// This publication with its bytes read from their owning source.
    fn project(self, tape: &Path) -> Publication {
        let (content, author) = project_body(self.body, tape);
        Publication {
            publication: self.publication,
            seq: self.seq,
            at: self.at,
            actor: self.actor,
            author,
            content,
            directed: self.directed,
            conversation: self.conversation,
        }
    }
}

/// A body's bytes read from their owning source, with the occupant whose
/// transcript authored an agent packet.
fn project_body(body: Body, tape: &Path) -> (Content, Option<CanonicalUuid>) {
    match body {
        Body::Published(RoomPublicationContent::Authored { text }) => {
            (Content::Text { text }, None)
        }
        Body::Published(RoomPublicationContent::Linked {
            link: RoomSource::Transcript(hunk),
        }) => (transcript(tape, hunk), None),
        Body::Published(RoomPublicationContent::Linked { link }) => {
            (Content::Linked { source: link }, None)
        }
        Body::Published(RoomPublicationContent::OopsGroomed { grooming }) => {
            (Content::OopsGroomed { grooming }, None)
        }
        Body::Messaged { hunk, author } => (transcript(tape, hunk), Some(author)),
        Body::Linked(source) => (Content::Linked { source }, None),
    }
}

/// The body of one conversation publication, before its bytes are read: an
/// operator post's text from its journal fact, a lane's from its transcript.
fn conversation_body(
    read: &journal::JournalRead<'_>,
    publication: &wire::rooms::RoomPublication,
) -> Result<Body, Failure> {
    let source = publication.source();
    if let RoomSource::Journal { fact } = source
        && let Some(seq) = Seq::new(fact.get())
        && let Some(posted) = read
            .room_fact(seq)
            .map_err(store)?
            .and_then(|envelope| envelope.fact.read::<RoomPosted>())
    {
        return Ok(Body::Published(RoomPublicationContent::Authored {
            text: posted.text,
        }));
    }
    Ok(Body::Published(RoomPublicationContent::Linked {
        link: source.clone(),
    }))
}

impl Publication {
    pub(crate) fn seq(&self) -> Seq {
        self.seq
    }

    pub(crate) fn publication(&self) -> &RoomAuthorityStamp {
        &self.publication
    }

    pub(crate) fn author_lane(&self) -> Option<CanonicalUuid> {
        match self.actor {
            Actor::Lane(lane) => Some(lane),
            _ => None,
        }
    }

    pub(crate) fn recipients(&self) -> Result<BTreeSet<CanonicalUuid>, String> {
        self.directed
            .iter()
            .map(|route| match &route.receipt {
                Receipt::Steering { lane, .. } => Ok(*lane),
                Receipt::Missing => Err(format!(
                    "room publication {} has no steering head for directive {}",
                    self.publication, route.directive
                )),
            })
            .collect()
    }
}

/// The hunk's bytes from its lane's durable tape, or why the tape cannot
/// supply them.
fn transcript(tape: &Path, hunk: TranscriptHunk) -> Content {
    match transcript_text(tape, &hunk) {
        Ok(text) => Content::Text { text },
        Err(unavailable) => Content::Unavailable {
            source: RoomSource::Transcript(hunk),
            unavailable,
        },
    }
}

fn transcript_text(tape: &Path, hunk: &TranscriptHunk) -> Result<String, RoomSourceUnavailable> {
    let dialect = spice::extension::registry()
        .ok()
        .and_then(|registry| registry.vendor(hunk.head.vendor.as_str()))
        .map(|vendor| vendor.dialect())
        .ok_or(RoomSourceUnavailable::Unreadable {})?;
    let root = tape
        .join(hunk.head.lane.to_string())
        .join(hunk.head.vendor.as_str());
    let decoded = spice::quote::decode_record(&root, hunk.head.source, hunk.locus, dialect)
        .map_err(|error| match error {
            spice::quote::RecordError::Missing => RoomSourceUnavailable::Missing {},
            spice::quote::RecordError::Unreadable => RoomSourceUnavailable::Unreadable {},
        })?;
    let event = usize::try_from(hunk.locus.event)
        .ok()
        .and_then(|event| decoded.events().get(event));
    let Some(listen::Event::StreamedProse(prose)) = event else {
        return Err(RoomSourceUnavailable::Changed {});
    };
    let range = usize::try_from(hunk.text.start)
        .ok()
        .zip(usize::try_from(hunk.text.end).ok());
    range
        .filter(|_| prose.role == listen::ProseRole::Assistant)
        .and_then(|(start, end)| prose.text.get(start..end))
        .map(str::to_owned)
        .ok_or(RoomSourceUnavailable::Changed {})
}

impl fmt::Display for RoomHistory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shown { room, anchor, page } => {
                write!(formatter, "room {room} anchor {anchor}")?;
                for publication in &page.rows {
                    write!(formatter, "\n{publication}")?;
                }
                match &page.omission {
                    Some(omission) => write!(formatter, "\n{omission}"),
                    None => Ok(()),
                }
            }
            Self::Exported {
                room,
                anchor,
                export,
            } => write!(formatter, "room {room} anchor {anchor}\n{export}"),
        }
    }
}

impl fmt::Display for Publication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} seq={} at={} from={}",
            self.publication, self.seq, self.at, self.actor
        )?;
        if let Some(author) = self.author {
            write!(formatter, " author={author}")?;
        }
        for route in &self.directed {
            formatter.write_str("\n  ")?;
            conversation::write_receipt(formatter, route.directive, &route.receipt)?;
        }
        if let Some(conversation) = &self.conversation {
            write!(formatter, "\n  {conversation}")?;
        }
        write_content(formatter, &self.content)
    }
}

/// A publication's bytes, one quoted line each, or what stands in for them.
fn write_content(formatter: &mut fmt::Formatter<'_>, content: &Content) -> fmt::Result {
    match content {
        Content::Text { text } => {
            for line in text.lines() {
                write!(formatter, "\n  | {line}")?;
            }
            Ok(())
        }
        Content::Linked { source } => write!(formatter, "\n  linked {}", Source(source)),
        Content::OopsGroomed { grooming } => {
            write!(
                formatter,
                "\n  oops {} cause={:?}",
                grooming.disposition, grooming.cause
            )?;
            for report in &grooming.reports {
                write!(formatter, "\n  report {report}")?;
            }
            if !grooming.exceptions.is_empty() {
                write!(formatter, "\n  exceptions {:?}", grooming.exceptions)?;
            }
            if !grooming.uncertainty.is_empty() {
                write!(formatter, "\n  uncertainty {:?}", grooming.uncertainty)?;
            }
            Ok(())
        }
        Content::Unavailable {
            source,
            unavailable,
        } => write!(
            formatter,
            "\n  unavailable {}: {}",
            Source(source),
            Unavailable(unavailable)
        ),
    }
}

/// A room source as one line names it.
struct Source<'a>(&'a RoomSource);

impl fmt::Display for Source<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            RoomSource::Transcript(hunk) => write!(
                formatter,
                "transcript lane={} vendor={} source={} member={} offset={} event={} bytes={}..{}",
                hunk.head.lane,
                hunk.head.vendor,
                hunk.head.source,
                hunk.locus.member,
                hunk.locus.offset,
                hunk.locus.event,
                hunk.text.start,
                hunk.text.end
            ),
            RoomSource::Journal { fact } => write!(formatter, "journal fact={fact}"),
            RoomSource::Artifact { digest } => write!(formatter, "artifact digest={digest}"),
        }
    }
}

struct Unavailable<'a>(&'a RoomSourceUnavailable);

impl fmt::Display for Unavailable<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            RoomSourceUnavailable::Missing {} => formatter.write_str("missing"),
            RoomSourceUnavailable::Hidden {} => formatter.write_str("hidden"),
            RoomSourceUnavailable::Changed {} => formatter.write_str("changed"),
            RoomSourceUnavailable::Unreadable {} => formatter.write_str("unreadable"),
            RoomSourceUnavailable::UnsupportedRetained { release } => {
                write!(formatter, "unsupported-retained release={release}")
            }
        }
    }
}

enum Failure {
    Refusal(Refusal),
    Fault(Fault),
}

impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        Self::Refusal(refusal)
    }
}

impl Failure {
    fn outcome<T>(self) -> Outcome<T> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }

    fn guard(self) -> journal::GuardError {
        match self {
            Self::Refusal(refusal) => journal::GuardError::Invalid(format!("{refusal:?}")),
            Self::Fault(fault) => {
                journal::GuardError::Invalid(format!("{}: {}", fault.reason, fault.detail))
            }
        }
    }
}

fn task_invalid(named: &str, error: &impl fmt::Display) -> Failure {
    Failure::Refusal(crate::read_page::refusal(
        "room",
        "task-invalid",
        Some(format!("{named}: {error}")),
        "read the board and pass one task handle",
    ))
}

fn store(error: journal::GuardError) -> Failure {
    fault(FaultClass::Store, "room::journal-unreadable", error)
}

fn fault(class: FaultClass, reason: &str, detail: impl fmt::Display) -> Failure {
    Failure::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail: detail.to_string(),
    })
}
