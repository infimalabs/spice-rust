//! The one path from a durable tape into accounted authority.
//!
//! Live capture and startup catch-up differ only in what wakes them, never in
//! how they read. Both hand a lane's tape to [`ingest`], which resumes from the
//! stored head, folds every complete record the scan yielded into one batch,
//! and accounts it. Keeping the read here is what makes the checkpoint mean
//! something: a second reader with its own resume rule would advance the same
//! head from a position this one never observed, and the spend between the two
//! positions would be accounted twice or not at all.
//!
//! Nothing in this module decides identity. A tape whose source generation is
//! missing or unreadable is refused rather than guessed at, because the only
//! forward migration for an identity-less tape belongs to its producer, which
//! adopts one through `tape::Tape::open`.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use journal::crossing::{
    Actor, CanonicalUuid, Instant, Origin, TranscriptSourceHead, TranscriptStreamKey, body,
};
use journal::{Applied, GuardError, Journal, JournalHead};
use listen::{Checkpoint, Dialect, Event, Position, RecordPayload, Scan, ScanError, SourcedEvent};
use tape::{SourceId, Tape};
use wire::Name;

use crate::account::{AccountError, Accounting, account};

/// Position every source generation is accounted from before its first batch.
const ORIGIN: body::TranscriptPosition = body::TranscriptPosition {
    member: 1,
    offset: 0,
};

/// One transcript to read, and the authorship its batch is recorded under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ingestion {
    /// Lane owning the transcript.
    pub lane: CanonicalUuid,
    /// Vendor that produced it.
    pub vendor: Name,
    /// Logical stream within that lane and vendor.
    pub stream: String,
    /// Directory holding the durable tape.
    pub root: PathBuf,
    /// Decoder contract the records are read under.
    pub dialect: Dialect,
    /// Actor credited with reading the transcript.
    pub actor: Actor,
    /// Origin the ingestion fact is threaded under.
    pub origin: Origin,
    /// Instant the batch is accounted at.
    pub at: Instant,
}

/// What one ingestion pass left behind.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "one pass returns one value that the caller consumes at once; the empty variant is the common answer and boxing the accounted one would cost every sweep an allocation"
)]
pub enum Ingested {
    /// No complete record was available, so no fact was written.
    ///
    /// This is the ordinary answer while a record is still being emitted: the
    /// trailing bytes have no delimiter yet, so they belong to no boundary the
    /// head could name. Writing an empty batch here would append a fact per
    /// poll that accounts nothing and advances the head to where it already
    /// stands.
    Unchanged,
    /// A batch crossed, leaving the head at the position it accounted to.
    Accounted {
        /// Journal outcome of the accounting transaction.
        applied: Applied,
        /// Exact durable batch whose commit instant can timestamp records that
        /// carried no vendor-authored time.
        batch: body::JournalIngested,
        /// Every complete record the batch accounted, in tape order.
        ///
        /// The pass already decoded these to fold the batch. Handing them back
        /// lets the caller present each one without a second read, and without
        /// depending on a per-record notification that may never have arrived.
        records: Vec<listen::Record>,
    },
}

/// Why one ingestion pass accounted nothing.
#[derive(Debug)]
pub enum IngestError {
    /// The stored head could not be read, so no resume point is known.
    Head(GuardError),
    /// The tape's source generation is missing, unreadable, or malformed.
    Identity {
        /// Tape whose identity could not be established.
        path: PathBuf,
        /// Typed tape failure.
        source: tape::Error,
    },
    /// The generation spelling could not cross between tape and authority.
    Spelling {
        /// Exact spelling that did not parse as a canonical generation.
        found: String,
    },
    /// The scan could not continue from its durable contract.
    Scan(ScanError),
    /// The batch was read but could not be accounted.
    Account(AccountError),
}

impl fmt::Display for IngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Head(error) => write!(formatter, "stored head could not be read: {error}"),
            Self::Identity { path, source } => write!(
                formatter,
                "tape {} has no readable source generation: {source}",
                path.display()
            ),
            Self::Spelling { found } => {
                write!(formatter, "source generation {found:?} is not canonical")
            }
            Self::Scan(error) => write!(formatter, "transcript could not be scanned: {error}"),
            Self::Account(error) => write!(formatter, "transcript could not be accounted: {error}"),
        }
    }
}

impl Error for IngestError {}

/// Read one transcript forward from its stored head and account what it holds.
///
/// The stored head alone decides where the read resumes and whether the
/// observed generation supersedes an earlier one. Both decisions are made again
/// by the accounting guard inside the transaction, so a head that moves between
/// this read and that commit refuses the batch rather than accounting it from a
/// position that is no longer current.
///
/// # Errors
///
/// Returns [`IngestError`] when the head cannot be read, the tape has no
/// readable identity, the scan cannot continue, or the batch is refused. Every
/// one of them leaves the stored head exactly as it was found.
pub fn ingest(journal: &mut Journal, ingestion: Ingestion) -> Result<Ingested, IngestError> {
    let key = TranscriptStreamKey::new(&ingestion.lane, &ingestion.vendor, &ingestion.stream);
    let stored = journal
        .read(|read| read.transcript_source_head(&key))
        .map_err(IngestError::Head)?
        .map(JournalHead::into_body);
    let observed = observe(&ingestion.root)?;
    let resume = Resume::decide(stored.as_ref(), &observed)?;
    let scan = listen::scan(
        &ingestion.root,
        ingestion.dialect,
        resume.checkpoint.as_ref(),
    )
    .map_err(IngestError::Scan)?;
    if scan.records.is_empty() {
        return Ok(Ingested::Unchanged);
    }
    let batch = fold(&ingestion, &resume, &scan);
    let retained = batch.clone();
    let applied = account(
        journal,
        Accounting {
            actor: ingestion.actor,
            origin: ingestion.origin,
            at: ingestion.at,
            batch,
        },
    )
    .map_err(IngestError::Account)?;
    Ok(Ingested::Accounted {
        applied,
        batch: retained,
        records: scan.records,
    })
}

/// The generation the tape currently publishes.
///
/// This is read before the scan rather than taken from it, because the stored
/// head has to be compared against the live generation to tell a continuation
/// from a replacement. Handing a stale checkpoint to the scan would only ever
/// produce a drift refusal, which cannot be told apart from real corruption.
fn observe(root: &Path) -> Result<SourceId, IngestError> {
    Tape::open_existing(root)
        .and_then(|tape| tape.source_id())
        .map_err(|source| IngestError::Identity {
            path: root.to_path_buf(),
            source,
        })
}

/// Where the read resumes, and what the resulting batch claims about identity.
struct Resume {
    checkpoint: Option<Checkpoint>,
    source: CanonicalUuid,
    from: body::TranscriptPosition,
    supersedes: Option<CanonicalUuid>,
}

impl Resume {
    /// Decide the resume point from the stored head and the live generation.
    ///
    /// The three arms are exactly the ones the accounting guard recognizes: no
    /// head at all, a head on this generation, and a head on a generation the
    /// tape no longer carries. Only the middle arm resumes; the other two read
    /// the generation from its origin, and the third additionally names the
    /// generation it retires so the guard can check the claim rather than infer
    /// it from a source that merely differs.
    fn decide(
        stored: Option<&TranscriptSourceHead>,
        observed: &SourceId,
    ) -> Result<Self, IngestError> {
        let source = canonical(observed)?;
        Ok(match stored {
            Some(head) if head.source == source => Self {
                checkpoint: Some(Checkpoint {
                    source: observed.clone(),
                    next: Position {
                        member: head.next.member,
                        offset: head.next.offset,
                    },
                }),
                source,
                from: head.next,
                supersedes: None,
            },
            Some(head) => Self {
                checkpoint: None,
                source,
                from: ORIGIN,
                supersedes: Some(head.source),
            },
            None => Self {
                checkpoint: None,
                source,
                from: ORIGIN,
                supersedes: None,
            },
        })
    }
}

/// Cross one generation spelling from tape into authority.
fn canonical(source: &SourceId) -> Result<CanonicalUuid, IngestError> {
    source.as_str().parse().map_err(|_| IngestError::Spelling {
        found: source.as_str().to_owned(),
    })
}

/// Fold every complete record the scan yielded into the one batch accounting it.
///
/// A record contributes to exactly one of the three tallies. Records carrying
/// usage contribute their samples; complete records carrying none, and complete
/// residue, are counted rather than dropped, so the batch accounts for every
/// boundary the head crossed and not merely the spending ones.
fn fold(ingestion: &Ingestion, resume: &Resume, scan: &Scan) -> body::JournalIngested {
    let mut usage = Vec::new();
    let mut non_usage_records = 0;
    let mut residue_records = 0;
    for record in &scan.records {
        match &record.payload {
            RecordPayload::Residue(_) => residue_records += 1,
            RecordPayload::Events(events) => {
                let before = usage.len();
                usage.extend(events.iter().filter_map(sample));
                if usage.len() == before {
                    non_usage_records += 1;
                }
            }
        }
    }
    body::JournalIngested {
        lane: ingestion.lane,
        vendor: ingestion.vendor.clone(),
        stream: ingestion.stream.clone(),
        source: resume.source,
        supersedes: resume.supersedes,
        from: resume.from,
        to: body::TranscriptPosition {
            member: scan.checkpoint.next.member,
            offset: scan.checkpoint.next.offset,
        },
        usage,
        non_usage_records,
        residue_records,
    }
}

/// The normalized sample one event carries, when it carries one.
fn sample(sourced: &SourcedEvent) -> Option<body::TranscriptUsage> {
    let Event::Usage(usage) = &sourced.event else {
        return None;
    };
    Some(body::TranscriptUsage {
        turns: usage.turns,
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        spans: sourced
            .spans
            .iter()
            .map(|span| body::TranscriptSpan {
                member: span.member,
                offset: span.offset,
                length: span.length,
            })
            .collect(),
        ordinal: sourced.ordinal,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use journal::crossing::{Actor, CanonicalUuid, FactKind, TranscriptSourceHead, body};
    use listen::{Dialect, ScanError};
    use tape::Tape;

    use super::{IngestError, Ingested, Ingestion, Journal, TranscriptStreamKey, ingest};

    const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116305";
    const PROSE: &[u8] =
        br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"working"}}"#;
    const USAGE: &[u8] = br#"{"type":"turn.completed","usage":{"input_tokens":11,"cached_input_tokens":7,"cache_write_input_tokens":3,"output_tokens":5,"reasoning_output_tokens":2}}"#;
    const RESIDUE: &[u8] = b"not-json";

    /// A pending record is only pending until its delimiter arrives.
    ///
    /// The trailing bytes belong to no boundary yet, so the head must stop
    /// short of them and the pass must write nothing at all rather than an
    /// empty batch. When the delimiter lands the remainder is accounted once,
    /// continuing from exactly the position the first pass stopped at.
    #[test]
    fn a_pending_record_leaves_the_head_where_the_last_complete_record_ended() {
        let scratch = Scratch::new("pending");
        let tape = Tape::open(scratch.path()).expect("tape opens");
        tape.tee(&line(PROSE)).expect("prose is durable");
        tape.tee(&line(RESIDUE)).expect("residue is durable");
        let complete = (line(PROSE).len() + line(RESIDUE).len()) as u64;
        tape.tee(&USAGE[..20]).expect("a partial record is durable");
        let journal = &mut authority();

        let first = accounted(journal, scratch.path());
        assert_eq!(first.to, position(1, complete), "pending bytes advanced it");
        assert!(first.usage.is_empty(), "no usage record was complete");
        assert_eq!((first.non_usage_records, first.residue_records), (1, 1));

        assert!(
            matches!(
                ingest(journal, ingestion(scratch.path())),
                Ok(Ingested::Unchanged)
            ),
            "a pass with no new complete record still wrote a batch",
        );
        assert_eq!(batches(journal).len(), 1, "the empty pass appended a fact");

        tape.tee(&line(&USAGE[20..])).expect("the record completes");
        let second = accounted(journal, scratch.path());
        assert_eq!(second.from, position(1, complete), "the replay lost bytes");
        assert_eq!(second.to, position(1, complete + line(USAGE).len() as u64));
        assert_eq!(second.usage.len(), 1, "the completed record carried usage");
        scratch.remove();
    }

    /// Sealing a member moves the head forward without re-reading the seal.
    ///
    /// A sealed member cannot grow, so the next record begins the member after
    /// it. The batch still continues from the position the head actually held,
    /// which is what proves the sealed member was not accounted a second time.
    #[test]
    fn a_sealed_member_carries_the_head_into_the_member_after_it() {
        let scratch = Scratch::new("sealing");
        let tape = Tape::open(scratch.path()).expect("tape opens");
        tape.tee(&line(PROSE)).expect("prose is durable");
        let journal = &mut authority();
        let first = accounted(journal, scratch.path());
        assert_eq!(first.to, position(1, line(PROSE).len() as u64));

        tape.seal().expect("the member seals");
        tape.tee(&line(USAGE)).expect("the next member grows");

        let second = accounted(journal, scratch.path());
        assert_eq!(second.from, first.to, "the batch skipped the sealed member");
        assert_eq!(second.to, position(2, line(USAGE).len() as u64));
        assert_eq!(second.usage.len(), 1, "the sealed-past record was lost");
        scratch.remove();
    }

    /// A generation replaced at the same path must be claimed, not assumed.
    ///
    /// The path is unchanged, so nothing about the location says the bytes are
    /// new. Only the generation says so, and the batch has to name the one it
    /// retires: a batch that merely arrived with a different source would be a
    /// silent restart abandoning every byte the old generation accounted.
    #[test]
    fn a_replacement_generation_supersedes_the_one_the_head_holds() {
        let scratch = Scratch::new("replacement");
        let root = scratch.path().join("selected");
        let first = Tape::open(&root).expect("the first generation opens");
        first.tee(&line(USAGE)).expect("the first generation grows");
        let journal = &mut authority();
        let opening = accounted(journal, &root);
        assert_eq!(opening.supersedes, None, "an opening batch retired nothing");

        fs::remove_dir_all(&root).expect("the first generation is removed");
        let replacement = Tape::open(&root).expect("the replacement opens");
        replacement
            .tee(&line(PROSE))
            .expect("the replacement grows");

        let restarted = accounted(journal, &root);
        assert_eq!(
            restarted.supersedes,
            Some(opening.source),
            "the replacement did not name the generation it retired",
        );
        assert_eq!(restarted.from, position(1, 0), "it resumed a dead position");
        assert_ne!(restarted.source, opening.source);
        assert_eq!(
            head(journal).expect("a head stands").source,
            restarted.source
        );
        scratch.remove();
    }

    /// Ingestion observes identity; it never invents one.
    ///
    /// An identity-less tape has no generation to account against, and reading
    /// one from the path or the file times would let two unrelated tapes share
    /// a checkpoint. The one forward migration belongs to the producer's own
    /// door, so the refusal stands until that door has been opened.
    #[test]
    fn an_identity_less_tape_is_refused_until_its_producer_adopts_one() {
        let scratch = Scratch::new("identity");
        let tape = Tape::open(scratch.path()).expect("tape opens");
        tape.tee(&line(USAGE)).expect("the record is durable");
        fs::remove_file(scratch.path().join("source")).expect("identity is removed");
        let journal = &mut authority();

        match ingest(journal, ingestion(scratch.path())) {
            Err(IngestError::Identity { .. }) => {}
            other => panic!("an identity-less tape was not refused: {other:?}"),
        }
        assert!(head(journal).is_none(), "a refused pass moved the head");

        Tape::open(scratch.path()).expect("the producer adopts an identity");
        let adopted = accounted(journal, scratch.path());
        assert_eq!(adopted.usage.len(), 1, "the adopted tape accounted nothing");
        scratch.remove();
    }

    /// Bytes that retreat behind the head are drift, not a shorter transcript.
    ///
    /// The stored position names bytes the generation is now too short to hold,
    /// so no honest continuation exists. Accounting from the origin instead
    /// would re-spend everything already accounted, so the pass refuses and
    /// leaves the head exactly where the last real batch put it.
    #[test]
    fn bytes_retreating_behind_the_head_refuse_without_moving_it() {
        let scratch = Scratch::new("retreat");
        let tape = Tape::open(scratch.path()).expect("tape opens");
        tape.tee(&line(USAGE)).expect("the record is durable");
        let journal = &mut authority();
        let opening = accounted(journal, scratch.path());

        fs::write(member(scratch.path()), b"short\n").expect("the member retreats");

        match ingest(journal, ingestion(scratch.path())) {
            Err(IngestError::Scan(ScanError::CheckpointRetreated { .. })) => {}
            other => panic!("a retreated member was not refused as drift: {other:?}"),
        }
        assert_eq!(head(journal).expect("a head stands").next, opening.to);
        assert_eq!(batches(journal).len(), 1, "the refusal appended a fact");
        scratch.remove();
    }

    /// A head standing past every member the tape offers is not a resume.
    ///
    /// The stored position names a member the tape no longer supplies at all,
    /// so no boundary exists to continue from. Reading the absence as a fresh
    /// tape would account the whole generation from the origin a second time,
    /// so the pass refuses and leaves the head where the last real batch put it.
    #[test]
    fn a_head_past_every_member_the_tape_supplies_refuses_without_moving_it() {
        let scratch = Scratch::new("gap");
        let tape = Tape::open(scratch.path()).expect("tape opens");
        tape.tee(&line(USAGE)).expect("the record is durable");
        let journal = &mut authority();
        let opening = accounted(journal, scratch.path());

        fs::remove_file(member(scratch.path())).expect("every member is withdrawn");

        match ingest(journal, ingestion(scratch.path())) {
            Err(IngestError::Scan(ScanError::CheckpointGap { .. })) => {}
            other => panic!("a head past every member was not refused: {other:?}"),
        }
        assert_eq!(head(journal).expect("a head stands").next, opening.to);
        assert_eq!(batches(journal).len(), 1, "the refusal appended a fact");
        scratch.remove();
    }

    fn authority() -> Journal {
        Journal::in_memory().expect("in-memory authority")
    }

    fn ingestion(root: &Path) -> Ingestion {
        Ingestion {
            lane: LANE.parse().expect("fixture lane"),
            vendor: "codex".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            root: root.to_path_buf(),
            dialect: Dialect::Codex,
            actor: Actor::Spice,
            origin: format!("lane:{LANE}").parse().expect("lane origin"),
            at: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        }
    }

    /// One pass that must have accounted a batch, returned as that batch.
    fn accounted(journal: &mut Journal, root: &Path) -> body::JournalIngested {
        match ingest(journal, ingestion(root)) {
            Ok(Ingested::Accounted { .. }) => batches(journal).pop().expect("the batch is stored"),
            other => panic!("the pass accounted nothing: {other:?}"),
        }
    }

    fn head(journal: &Journal) -> Option<TranscriptSourceHead> {
        let vendor = "codex".parse().expect("fixture vendor");
        let lane: CanonicalUuid = LANE.parse().expect("fixture lane");
        journal
            .read(|read| {
                read.transcript_source_head(&TranscriptStreamKey::new(&lane, &vendor, "session"))
            })
            .expect("read the stored head")
            .map(journal::JournalHead::into_body)
    }

    fn batches(journal: &Journal) -> Vec<body::JournalIngested> {
        journal
            .fact_snapshot()
            .expect("fact snapshot")
            .facts()
            .iter()
            .filter(|envelope| envelope.fact.kind() == FactKind::JournalIngested)
            .map(|envelope| envelope.fact.read().expect("the batch reads back"))
            .collect()
    }

    const fn position(member: u64, offset: u64) -> body::TranscriptPosition {
        body::TranscriptPosition { member, offset }
    }

    fn line(bytes: &[u8]) -> Vec<u8> {
        let mut line = bytes.to_vec();
        line.push(b'\n');
        line
    }

    /// The one plaintext member a single-member fixture tape holds.
    fn member(root: &Path) -> PathBuf {
        fs::read_dir(root)
            .expect("the tape root lists")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "plain")
            })
            .expect("a growing tape has one plaintext member")
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "spice-lane-ingest-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("scratch root is exclusively created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn remove(self) {
            fs::remove_dir_all(&self.0).expect("scratch root is explicitly removed");
            std::mem::forget(self);
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
