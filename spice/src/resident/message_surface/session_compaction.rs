//! Structured compaction boundaries recovered from preserved vendor sessions.

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

use super::*;

mod file_stamp;
mod history_authentication;
mod history_cards;
mod identity;
mod presentation;
use file_stamp::*;
#[cfg(test)]
use history_authentication::AFTER_HISTORY_AUTHENTICATION_READ;
pub(super) use history_authentication::{HistoryCheckpoint, history_checkpoint};
use history_authentication::{admit_history_source, read_authenticated_history};
#[cfg(test)]
use history_cards::history_window;
pub(super) use history_cards::{HistoryRequest, HistoryScan, SessionCards, retained_messages};
pub(super) use identity::session_member;
use presentation::publish;

/// Per-resident cursors over canonical session JSONL files.
///
/// Projects the typed compaction events a lane's preserved session records
/// into the app, folding adjacent tool evidence into each boundary.
pub(in crate::resident) struct SessionCompactions {
    environment: crate::session_command::Environment,
    cursors: BTreeMap<CanonicalUuid, Cursor>,
    reported: BTreeMap<CanonicalUuid, ReportKey>,
}

#[derive(Debug, Eq, PartialEq)]
enum ReportKey {
    Resolution(String),
    Scan {
        path: PathBuf,
        source: FileObservation,
        class: String,
    },
}

struct Cursor {
    thread: String,
    path: PathBuf,
    dialect: listen::Dialect,
    author: CanonicalUuid,
    member: u64,
    initialized: bool,
    next: u64,
    prefix_start: u64,
    next_call: u64,
    pending: BTreeMap<String, PendingTool>,
    prefix: blake3::Hasher,
    guard: Option<RecordGuard>,
    validation: Option<PrefixValidation>,
    failure: Option<FileObservation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileObservation {
    Present(FileStamp),
    Unavailable {
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
}

impl FileObservation {
    fn read(path: &Path) -> Self {
        File::open(path)
            .and_then(|file| FileStamp::read(&file))
            .map_or_else(
                |error| Self::Unavailable {
                    kind: error.kind(),
                    raw_os_error: error.raw_os_error(),
                },
                Self::Present,
            )
    }
}

enum ScanAttempt {
    Complete(Vec<CompactionRecord>),
    Latched,
    Failed {
        error: io::Error,
        source: FileObservation,
        class: String,
    },
}

struct PendingTool {
    order: u64,
    preview: String,
    /// The typed intent the call declared, which its updates do not repeat.
    intent: Option<String>,
}

struct RecordGuard {
    offset: u64,
    length: u64,
    hash: u64,
}

const RECORD_GUARD_BYTES: usize = 256;
const SCAN_BYTE_BUDGET: usize = 256 * 1024;
const SCAN_RECORD_BUDGET: usize = 128;
const VALIDATION_BYTE_BUDGET: usize = 256 * 1024;
const AUTHENTICATION_BYTE_BUDGET: usize = VALIDATION_BYTE_BUDGET * 2;
/// Fixed allowances for one reverse history window.
pub(super) const HISTORY_BYTE_BUDGET: usize = 256 * 1024;
const HISTORY_RECORD_BUDGET: usize = 128;
const HISTORY_PASS_BUDGET: u32 = 1;
/// Windows one history request may chain, sized from the shared
/// [`HISTORY_REQUEST_BYTES`] span so this half of a merged page reaches as
/// far back per request as the tape half does.
const HISTORY_WINDOW_LIMIT: usize = (HISTORY_REQUEST_BYTES / HISTORY_BYTE_BUDGET as u64) as usize;
/// Preserved compaction boundaries materialized before the app opens.
///
/// Boundaries are not messages and do not satisfy [`wire::MESSAGE_FLOOR`];
/// this is how many of them the initial view can need, one between each of an
/// agent's newest messages and one above them.
const BOOTSTRAP_BOUNDARY_LIMIT: usize = wire::MESSAGE_FLOOR + 1;

struct PrefixValidation {
    start: u64,
    length: u64,
    expected: blake3::Hash,
    offset: u64,
    observed: blake3::Hasher,
    stamp: FileStamp,
    authentication: Option<PrefixAuthentication>,
    records: Vec<CompactionRecord>,
    #[cfg(test)]
    after_authentication_read: Option<Box<dyn FnOnce()>>,
}

struct PrefixAuthentication {
    start: u64,
    length: u64,
    expected: blake3::Hash,
    offset: u64,
    observed: blake3::Hasher,
    stamp: FileStamp,
}

#[cfg(test)]
thread_local! {
    static VALIDATION_BYTES_READ: Cell<u64> = const { Cell::new(0) };
    static BOOTSTRAP_BYTES_DECODED: Cell<u64> = const { Cell::new(0) };
}

impl SessionCompactions {
    pub(in crate::resident) fn new(environment: crate::session_command::Environment) -> Self {
        Self {
            environment,
            cursors: BTreeMap::new(),
            reported: BTreeMap::new(),
        }
    }

    /// Record and publish every newly completed structured boundary for current sessions.
    pub(in crate::resident) fn synchronize(
        &mut self,
        selection: (&wire::ResidencyHead, CanonicalUuid),
        publisher: &impl AgentMessagePublisher,
        state: &Mutex<MessageState>,
        diagnostic: &mut dyn io::Write,
    ) -> CommandResult<()> {
        let (residency, lane) = selection;
        self.synchronize_selected_with(residency, Some(lane), diagnostic, |record, lane, author| {
            publish(record, lane, author, publisher, &mut *lock_state(state)?)
        })
    }

    fn synchronize_selected_with<P>(
        &mut self,
        residency: &wire::ResidencyHead,
        selected: Option<CanonicalUuid>,
        diagnostic: &mut dyn io::Write,
        mut present: P,
    ) -> CommandResult<()>
    where
        P: FnMut(CompactionRecord, CanonicalUuid, CanonicalUuid) -> CommandResult<()>,
    {
        let current = residency
            .lanes
            .iter()
            .filter_map(|lane| lane.session.as_ref().map(|session| (lane.lane, session)))
            .collect::<BTreeMap<_, _>>();
        let lanes = current.keys().copied().collect::<BTreeSet<_>>();
        self.cursors.retain(|lane, _| lanes.contains(lane));
        self.reported.retain(|lane, _| lanes.contains(lane));

        for (lane, session) in current
            .into_iter()
            .filter(|(lane, _)| selected.is_none_or(|selected| selected == *lane))
        {
            self.ensure_cursor(lane, session, diagnostic);
            let Some((scanned, path, author)) = self.cursors.get_mut(&lane).map(|cursor| {
                let path = cursor.path.clone();
                let author = cursor.author;
                (scan_once_per_source(cursor), path, author)
            }) else {
                continue;
            };
            match scanned {
                ScanAttempt::Complete(records) => {
                    for record in records {
                        app_diagnostic(present(record, lane, author), diagnostic)?;
                    }
                }
                ScanAttempt::Latched => {}
                ScanAttempt::Failed {
                    error,
                    source,
                    class,
                } => self.report_scan_once(lane, &path, source, class, error, diagnostic),
            }
        }
        Ok(())
    }

    fn ensure_cursor(
        &mut self,
        lane: CanonicalUuid,
        session: &wire::LaneSession,
        diagnostic: &mut dyn io::Write,
    ) {
        if let Some(cursor) = self.cursors.get_mut(&lane)
            && cursor.thread == session.identity
        {
            cursor.author = session.occupant;
            return;
        }
        match crate::session_command::vendor_thread_transcript(
            &session.identity,
            &session.vendor,
            std::path::Path::new(&session.worktree),
            &self.environment,
        ) {
            Ok((path, dialect)) => {
                self.reported.remove(&lane);
                self.cursors.insert(
                    lane,
                    Cursor::new(session.identity.clone(), path, dialect, session.occupant),
                );
            }
            Err(error) => self.report_once(
                lane,
                ReportKey::Resolution(error.clone()),
                error,
                diagnostic,
            ),
        }
    }

    fn report_scan_once(
        &mut self,
        lane: CanonicalUuid,
        path: &Path,
        source: FileObservation,
        class: String,
        error: io::Error,
        diagnostic: &mut dyn io::Write,
    ) {
        self.report_once(
            lane,
            ReportKey::Scan {
                path: path.to_path_buf(),
                source,
                class,
            },
            format!("{}: {error}", path.display()),
            diagnostic,
        );
    }

    fn report_once(
        &mut self,
        lane: CanonicalUuid,
        key: ReportKey,
        detail: String,
        diagnostic: &mut dyn io::Write,
    ) {
        if self.reported.get(&lane) == Some(&key) {
            return;
        }
        self.reported.insert(lane, key);
        let _ = writeln!(
            diagnostic,
            "session compaction unavailable for lane {lane}: {detail}",
        );
    }
}

impl Cursor {
    fn new(thread: String, path: PathBuf, dialect: listen::Dialect, author: CanonicalUuid) -> Self {
        Self {
            member: session_member(&thread),
            thread,
            path,
            dialect,
            author,
            initialized: false,
            next: 0,
            prefix_start: 0,
            next_call: 0,
            pending: BTreeMap::new(),
            prefix: blake3::Hasher::new(),
            guard: None,
            validation: None,
            failure: None,
        }
    }
}

fn scan_once_per_source(cursor: &mut Cursor) -> ScanAttempt {
    let observed = FileObservation::read(&cursor.path);
    if cursor.failure == Some(observed) {
        return ScanAttempt::Latched;
    }
    if cursor.failure.take().is_some() && !cursor.initialized {
        reset(cursor);
    }
    match scan(cursor) {
        Ok(records) => ScanAttempt::Complete(records),
        Err(error) => {
            let source = FileObservation::read(&cursor.path);
            let class = scan_error_class(&error);
            cursor.failure = Some(source);
            ScanAttempt::Failed {
                error,
                source,
                class,
            }
        }
    }
}

fn scan_error_class(error: &io::Error) -> String {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<HistoryExhaustion>())
        .map_or_else(
            || format!("io::{:?}", error.kind()),
            |error| {
                if error.oversized_record {
                    "history-record-oversized".to_owned()
                } else {
                    "history-work-exhausted".to_owned()
                }
            },
        )
}

struct CompactionRecord {
    decoded: listen::Event,
    authored: String,
    ordinal: u32,
    unavailable_tool_terminals: Vec<String>,
}

impl CompactionRecord {
    fn locus(&self) -> listen::Locus {
        let listen::Event::Compaction(compaction) = &self.decoded else {
            unreachable!("only compaction events become compaction records")
        };
        compaction.locus
    }
}

fn scan(cursor: &mut Cursor) -> io::Result<Vec<CompactionRecord>> {
    if !cursor.initialized {
        return bootstrap(cursor, BOOTSTRAP_BOUNDARY_LIMIT);
    }
    if cursor.validation.is_some() {
        return advance_validation(cursor);
    }
    let mut file = File::open(&cursor.path)?;
    if file.metadata()?.len() < cursor.next || !guard_matches(&mut file, cursor)? {
        reset(cursor);
    }
    file.seek(SeekFrom::Start(cursor.next))?;
    let mut reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut bytes = Vec::new();
    let mut scanned = 0_usize;
    for _ in 0..SCAN_RECORD_BUDGET {
        bytes.clear();
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 || bytes.last() != Some(&b'\n') {
            break;
        }
        scanned = scanned.saturating_add(read);
        decode_record(cursor, &bytes, &mut records)?;
        if !records.is_empty() || scanned >= SCAN_BYTE_BUDGET {
            break;
        }
    }
    if records.is_empty() {
        return Ok(records);
    }
    let stamp = FileStamp::read(reader.get_ref())?;
    drop(reader);
    cursor.validation = Some(PrefixValidation {
        start: cursor.prefix_start,
        length: cursor.next,
        expected: cursor.prefix.clone().finalize(),
        offset: cursor.prefix_start,
        observed: blake3::Hasher::new(),
        stamp,
        authentication: None,
        records,
        #[cfg(test)]
        after_authentication_read: None,
    });
    advance_validation(cursor)
}

/// Seed a new resident from the bounded initial visible tail the app presents.
///
/// A forward replay from byte zero makes the hundredth boundary pay for the
/// first ninety-nine prefixes again. Read one fixed newest-record page, retain
/// its older continuation, and continue live scanning from that snapshot.
fn bootstrap(cursor: &mut Cursor, visible_limit: usize) -> io::Result<Vec<CompactionRecord>> {
    let checkpoint = history_checkpoint(&cursor.path)?;
    let page = bootstrap_before(cursor, visible_limit, &checkpoint, HistoryRead::Cursor)?;
    Ok(page.records)
}

struct HistoryPage {
    records: Vec<CompactionRecord>,
    start: u64,
    work: HistoryWork,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HistoryWork {
    pub(super) source_bytes: u64,
    pub(super) decoded_records: u32,
    pub(super) passes: u32,
    pub(super) elapsed: Duration,
}

impl HistoryWork {
    /// One window stays inside its fixed allowances.
    pub(super) fn within_limits(self) -> bool {
        self.source_bytes <= HISTORY_BYTE_BUDGET as u64
            && self.decoded_records <= HISTORY_RECORD_BUDGET as u32
            && self.passes <= HISTORY_PASS_BUDGET
    }

    fn sampled(mut self, started: Instant) -> Self {
        self.elapsed = started.elapsed();
        self
    }

    fn joined(self, other: Self) -> Self {
        Self {
            source_bytes: self.source_bytes.saturating_add(other.source_bytes),
            decoded_records: self.decoded_records.saturating_add(other.decoded_records),
            passes: self.passes.saturating_add(other.passes),
            elapsed: self.elapsed.saturating_add(other.elapsed),
        }
    }
}

/// What one reverse window serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryRead {
    /// Seat the live cursor on the newest record boundary the window reaches.
    Cursor,
    /// Serve older records on demand; the cursor is discarded afterwards.
    Page,
}

fn bootstrap_before(
    cursor: &mut Cursor,
    visible_limit: usize,
    checkpoint: &HistoryCheckpoint,
    read: HistoryRead,
) -> io::Result<HistoryPage> {
    let started = Instant::now();
    let source = read_history_source(&cursor.path, checkpoint, started, read)?;
    let selected = selected_history_start(&source.bytes);
    let seat = source.aligned.saturating_add(selected as u64);
    let start = source.older.saturating_add(selected as u64);
    prepare_history_cursor(cursor, seat);
    let mut work = source.work;
    let records = decode_history_records(
        cursor,
        &source.bytes[selected..],
        visible_limit,
        source.stamp,
        started,
        &mut work,
    )?;
    work.elapsed = started.elapsed();
    if !work.within_limits() {
        return Err(history_budget_exhausted(work));
    }
    cursor.initialized = true;
    cursor.prefix_start = cursor.next;
    cursor.prefix = blake3::Hasher::new();
    Ok(HistoryPage {
        records,
        start,
        work,
    })
}

struct HistorySource {
    stamp: FileStamp,
    /// Offset of the first byte of `bytes`, where the cursor is seated; a
    /// window skipped as the inside of one long record seats it at the
    /// boundary the window reached.
    aligned: u64,
    /// Where the window behind this one ends: the first aligned record, or
    /// the window start when the whole window lay inside one record.
    older: u64,
    bytes: Vec<u8>,
    work: HistoryWork,
}

/// How one reverse window lines up with the record delimiters it holds.
enum HistoryAlignment {
    /// `bytes` begins a complete record at this offset.
    Record(u64),
    /// The window lies inside one record longer than itself; `terminated`
    /// says that record ends exactly at the window end.
    Inside { terminated: bool },
}

fn read_history_source(
    path: &PathBuf,
    checkpoint: &HistoryCheckpoint,
    started: Instant,
    read: HistoryRead,
) -> io::Result<HistorySource> {
    let end = checkpoint.next();
    let source_bytes = end.min(HISTORY_BYTE_BUDGET as u64);
    let source_start = end.saturating_sub(source_bytes);
    let (stamp, mut bytes) = read_authenticated_history(path, checkpoint, source_start, end)?;
    let work = HistoryWork {
        source_bytes,
        decoded_records: 0,
        passes: 1,
        elapsed: started.elapsed(),
    };
    if !work.within_limits() {
        return Err(history_budget_exhausted(work));
    }
    let (aligned, older) = match align_history_source(&mut bytes, source_start, end) {
        HistoryAlignment::Record(aligned) => (aligned, aligned),
        // The record is longer than one window, so it is skipped: a page
        // keeps reading behind it, and a cursor seats on the boundary it
        // ends at. An open record has no such boundary yet, so a cursor
        // waits for its delimiter instead of resuming inside it.
        HistoryAlignment::Inside { terminated: true } => (end, source_start),
        HistoryAlignment::Inside { terminated: false } if read == HistoryRead::Page => {
            (source_start, source_start)
        }
        HistoryAlignment::Inside { terminated: false } => {
            return Err(history_record_exceeds_budget(work.sampled(started)));
        }
    };
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1);
    bytes.truncate(complete);
    Ok(HistorySource {
        stamp,
        aligned,
        older,
        bytes,
        work,
    })
}

fn align_history_source(bytes: &mut Vec<u8>, source_start: u64, end: u64) -> HistoryAlignment {
    if source_start == 0 {
        return HistoryAlignment::Record(0);
    }
    let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
        bytes.clear();
        return HistoryAlignment::Inside { terminated: false };
    };
    let aligned = source_start.saturating_add(newline as u64 + 1);
    if aligned == end {
        bytes.clear();
        return HistoryAlignment::Inside { terminated: true };
    }
    bytes.drain(..=newline);
    HistoryAlignment::Record(aligned)
}

fn selected_history_start(bytes: &[u8]) -> usize {
    let mut starts = vec![0_usize];
    starts.extend(
        bytes
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1)),
    );
    starts.pop();
    let selected = starts.len().saturating_sub(HISTORY_RECORD_BUDGET);
    starts.get(selected).copied().unwrap_or(0)
}

fn prepare_history_cursor(cursor: &mut Cursor, start: u64) {
    cursor.next = start;
    cursor.prefix_start = start;
    cursor.next_call = 0;
    cursor.pending.clear();
    cursor.prefix = blake3::Hasher::new();
    cursor.guard = None;
    cursor.validation = None;
}

fn decode_history_records(
    cursor: &mut Cursor,
    bytes: &[u8],
    visible_limit: usize,
    stamp: FileStamp,
    started: Instant,
    work: &mut HistoryWork,
) -> io::Result<Vec<CompactionRecord>> {
    let mut records = Vec::new();
    let mut visible = Vec::new();
    for record in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !work.within_limits() {
            return Err(history_budget_exhausted(work.sampled(started)));
        }
        #[cfg(test)]
        BOOTSTRAP_BYTES_DECODED.with(|count| {
            count.set(count.get().saturating_add(record.len() as u64));
        });
        let offset = cursor.next;
        let count = decode_record(cursor, record, &mut records)
            .map_err(|error| finish_failed_history_decode(cursor, stamp, error))?;
        visible.push((offset, count));
        work.decoded_records = work.decoded_records.saturating_add(1);
    }
    retain_visible_history(&mut records, &visible, visible_limit, cursor.next);
    Ok(records)
}

fn finish_failed_history_decode(
    cursor: &mut Cursor,
    stamp: FileStamp,
    error: io::Error,
) -> io::Error {
    let unchanged = File::open(&cursor.path)
        .and_then(|file| FileStamp::read(&file))
        .is_ok_and(|observed| observed == stamp);
    if unchanged {
        cursor.initialized = true;
        cursor.prefix_start = cursor.next;
        cursor.prefix = blake3::Hasher::new();
    } else {
        reset(cursor);
    }
    error
}

fn retain_visible_history(
    records: &mut Vec<CompactionRecord>,
    visible: &[(u64, usize)],
    visible_limit: usize,
    end: u64,
) {
    if visible_limit == 0 {
        return;
    }
    let mut count = 0_usize;
    let mut start = end;
    for (offset, record_count) in visible.iter().rev() {
        start = *offset;
        count = count.saturating_add(*record_count);
        if count >= visible_limit {
            break;
        }
    }
    records.retain(|record| record.locus().offset >= start);
}

#[derive(Debug)]
struct HistoryExhaustion {
    work: HistoryWork,
    oversized_record: bool,
}

impl std::fmt::Display for HistoryExhaustion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.oversized_record {
            return write!(
                formatter,
                "preserved session record runs at least {} bytes without a delimiter, past the fixed {HISTORY_BYTE_BUDGET}-byte history allowance, after {} bytes, {} records, {} passes, and {:?}",
                self.work.source_bytes,
                self.work.source_bytes,
                self.work.decoded_records,
                self.work.passes,
                self.work.elapsed,
            );
        }
        write!(
            formatter,
            "preserved session history page exhausted its fixed work allowance after {} bytes, {} records, {} passes, and {:?}",
            self.work.source_bytes, self.work.decoded_records, self.work.passes, self.work.elapsed,
        )
    }
}

impl std::error::Error for HistoryExhaustion {}

fn history_budget_exhausted(work: HistoryWork) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        HistoryExhaustion {
            work,
            oversized_record: false,
        },
    )
}

fn history_record_exceeds_budget(work: HistoryWork) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        HistoryExhaustion {
            work,
            oversized_record: true,
        },
    )
}

fn decode_record(
    cursor: &mut Cursor,
    bytes: &[u8],
    records: &mut Vec<CompactionRecord>,
) -> io::Result<usize> {
    let length = u64::try_from(bytes.len()).expect("a record buffer length fits u64");
    let locus = listen::Locus {
        member: cursor.member,
        offset: cursor.next,
        length,
    };
    let offset = cursor.next;
    cursor.next = cursor.next.saturating_add(length);
    cursor.prefix.update(bytes);
    cursor.guard = Some(RecordGuard {
        offset,
        length,
        hash: record_guard_hash(bytes),
    });
    let mut decoded = listen::decode(cursor.dialect, bytes, locus)
        .expect("the locus is built from the exact session record bytes");
    crate::session_media::resolve(&mut decoded, &cursor.path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let authored = decoded
        .context()
        .and_then(|context| context.timestamp.clone());
    let mut visible = 0_usize;
    for (ordinal, event) in decoded.events().iter().enumerate() {
        visible = visible.saturating_add(usize::from(visible_event(event)));
        let ordinal = u32::try_from(ordinal)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        match event {
            listen::Event::Tool(tool) => fold_tool(cursor, tool),
            listen::Event::Compaction(_) => {
                let Some(authored) = authored.clone() else {
                    cursor.pending.clear();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "preserved session compaction record carries no timestamp",
                    ));
                };
                let mut unavailable = cursor.pending.values().collect::<Vec<_>>();
                unavailable.sort_by_key(|tool| tool.order);
                records.push(CompactionRecord {
                    decoded: event.clone(),
                    authored,
                    ordinal,
                    unavailable_tool_terminals: unavailable
                        .into_iter()
                        .map(|tool| tool.preview.clone())
                        .collect(),
                });
                cursor.pending.clear();
            }
            _ => {}
        }
    }
    Ok(visible)
}

fn visible_event(event: &listen::Event) -> bool {
    match event {
        listen::Event::Prose(_) => spice::agent_prose::quoteable(event).is_some(),
        listen::Event::Compaction(_) => true,
        listen::Event::Tool(_) if spice::agent_prose::quoteable(event).is_some() => true,
        listen::Event::Tool(tool) => match &tool.moment {
            listen::ToolMoment::Result(outcome) => !outcome.media.is_empty(),
            listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => false,
        },
        _ => false,
    }
}

fn fold_tool(cursor: &mut Cursor, tool: &listen::Tool) {
    let (name, arguments, declared) = match &tool.moment {
        listen::ToolMoment::Call {
            name,
            arguments,
            intent,
        } => (
            name,
            arguments,
            Some(super::tool_preview::declared_intent(intent.as_deref())),
        ),
        listen::ToolMoment::Update { name, arguments } => (name, arguments, None),
        listen::ToolMoment::Result(_) => {
            cursor.pending.remove(&tool.call_id);
            return;
        }
    };
    let pending = cursor.pending.get_mut(&tool.call_id);
    let intent = declared.unwrap_or_else(|| pending.as_ref().and_then(|tool| tool.intent.clone()));
    let preview = super::tool_preview::call_preview(name, arguments, intent.as_deref());
    if let Some(pending) = pending {
        pending.preview = preview;
        pending.intent = intent;
    } else {
        let order = cursor.next_call;
        cursor.next_call = cursor.next_call.saturating_add(1);
        cursor.pending.insert(
            tool.call_id.clone(),
            PendingTool {
                order,
                preview,
                intent,
            },
        );
    }
}

fn reset(cursor: &mut Cursor) {
    cursor.initialized = false;
    cursor.next = 0;
    cursor.prefix_start = 0;
    cursor.next_call = 0;
    cursor.pending.clear();
    cursor.prefix = blake3::Hasher::new();
    cursor.guard = None;
    cursor.validation = None;
}

fn advance_validation(cursor: &mut Cursor) -> io::Result<Vec<CompactionRecord>> {
    let validation = cursor
        .validation
        .as_mut()
        .expect("prefix validation advances only while pending");
    let mut file = File::open(&cursor.path)?;
    let stamp = FileStamp::read(&file)?;
    if validation.authentication.is_none() {
        let Some(extension) = validation.stamp.extension(stamp, validation.length) else {
            reset(cursor);
            return Ok(Vec::new());
        };
        if extension != validation.stamp {
            begin_prefix_authentication(validation, extension);
        }
    }
    if validation.authentication.is_some() {
        match advance_prefix_authentication(&mut file, validation)? {
            AuthenticationProgress::Pending => {}
            AuthenticationProgress::Moved => return Ok(Vec::new()),
            AuthenticationProgress::Complete => {}
            AuthenticationProgress::Invalid => {
                reset(cursor);
                return Ok(Vec::new());
            }
        }
    }
    if validation.authentication.is_some() && validation.offset == validation.length {
        return Ok(Vec::new());
    }
    let main_stamp = validation
        .authentication
        .as_ref()
        .map_or(validation.stamp, |authentication| authentication.stamp);
    let observed_stamp = FileStamp::read(&file)?;
    let Some(observed_stamp) = main_stamp.extension(observed_stamp, validation.length) else {
        reset(cursor);
        return Ok(Vec::new());
    };
    if observed_stamp != main_stamp {
        begin_prefix_authentication(validation, observed_stamp);
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(validation.offset))?;
    let remaining = validation.length.saturating_sub(validation.offset);
    let limit = usize::try_from(remaining.min(VALIDATION_BYTE_BUDGET as u64))
        .expect("the bounded validation read fits usize");
    let mut bytes = vec![0; limit];
    if read_validation_exact(&mut file, &mut bytes).is_err() {
        reset(cursor);
        return Ok(Vec::new());
    }
    let mut observed = validation.observed.clone();
    observed.update(&bytes);
    let offset = validation
        .offset
        .saturating_add(u64::try_from(bytes.len()).expect("a validation buffer fits u64"));
    let stamp = FileStamp::read(&file)?;
    let Some(extension) = main_stamp.extension(stamp, validation.length) else {
        reset(cursor);
        return Ok(Vec::new());
    };
    if extension != main_stamp {
        begin_prefix_authentication(validation, extension);
        return Ok(Vec::new());
    }
    validation.observed = observed;
    validation.offset = offset;
    if let Some(authentication) = validation.authentication.as_mut() {
        authentication.length = validation.offset;
        authentication.expected = validation.observed.clone().finalize();
    }
    if validation.offset < validation.length {
        return Ok(Vec::new());
    }
    if validation.authentication.is_some() {
        return Ok(Vec::new());
    }
    if validation.observed.finalize() != validation.expected {
        reset(cursor);
        return Ok(Vec::new());
    }
    let records = cursor
        .validation
        .take()
        .expect("completed validation remains pending")
        .records;
    cursor.prefix_start = cursor.next;
    cursor.prefix = blake3::Hasher::new();
    Ok(records)
}

enum AuthenticationProgress {
    Pending,
    Moved,
    Complete,
    Invalid,
}

fn begin_prefix_authentication(validation: &mut PrefixValidation, stamp: FileStamp) {
    if validation.offset == validation.start {
        validation.stamp = stamp;
        return;
    }
    validation.authentication = Some(PrefixAuthentication {
        start: validation.start,
        length: validation.offset,
        expected: validation.observed.clone().finalize(),
        offset: validation.start,
        observed: blake3::Hasher::new(),
        stamp,
    });
}

fn advance_prefix_authentication(
    file: &mut File,
    validation: &mut PrefixValidation,
) -> io::Result<AuthenticationProgress> {
    let authentication = validation
        .authentication
        .as_ref()
        .expect("extension authentication advances only while pending");
    let stamp = FileStamp::read(file)?;
    let Some(stamp) = authentication.stamp.extension(stamp, validation.length) else {
        return Ok(AuthenticationProgress::Invalid);
    };
    if stamp != authentication.stamp {
        // Metadata cannot distinguish append from rewrite-plus-append. Keep
        // primary progress, but authenticate it again under one stable stamp
        // before any held record can publish.
        begin_prefix_authentication(validation, stamp);
        return Ok(AuthenticationProgress::Pending);
    }
    let remaining = authentication.length.saturating_sub(authentication.offset);
    let limit = usize::try_from(remaining.min(AUTHENTICATION_BYTE_BUDGET as u64))
        .expect("the bounded authentication read fits usize");
    file.seek(SeekFrom::Start(authentication.offset))?;
    let mut bytes = vec![0; limit];
    if read_validation_exact(file, &mut bytes).is_err() {
        return Ok(AuthenticationProgress::Invalid);
    }

    #[cfg(test)]
    if let Some(after_read) = validation.after_authentication_read.take() {
        after_read();
    }

    let post = FileStamp::read(file)?;
    let Some(post) = stamp.extension(post, validation.length) else {
        return Ok(AuthenticationProgress::Invalid);
    };
    if post != stamp {
        let authentication = validation
            .authentication
            .as_mut()
            .expect("moving authentication remains pending");
        authentication.offset = authentication.start;
        authentication.observed = blake3::Hasher::new();
        authentication.stamp = post;
        return Ok(AuthenticationProgress::Moved);
    }

    let authentication = validation
        .authentication
        .as_mut()
        .expect("stable authentication remains pending");
    authentication.stamp = post;
    authentication.observed.update(&bytes);
    authentication.offset = authentication
        .offset
        .saturating_add(u64::try_from(bytes.len()).expect("an authentication buffer fits u64"));
    if authentication.offset < authentication.length {
        return Ok(AuthenticationProgress::Pending);
    }
    if authentication.observed.finalize() != authentication.expected {
        return Ok(AuthenticationProgress::Invalid);
    }
    validation.stamp = authentication.stamp;
    validation.authentication = None;
    Ok(AuthenticationProgress::Complete)
}

fn read_validation_exact(file: &mut File, bytes: &mut [u8]) -> io::Result<()> {
    #[cfg(test)]
    VALIDATION_BYTES_READ.with(|read| {
        read.set(
            read.get()
                .saturating_add(u64::try_from(bytes.len()).expect("a validation read fits u64")),
        );
    });
    file.read_exact(bytes)
}

#[cfg(test)]
fn reset_validation_bytes_read() {
    VALIDATION_BYTES_READ.with(|read| read.set(0));
}

#[cfg(test)]
fn validation_bytes_read() -> u64 {
    VALIDATION_BYTES_READ.with(Cell::get)
}

fn guard_matches(file: &mut File, cursor: &Cursor) -> io::Result<bool> {
    let Some(guard) = &cursor.guard else {
        return Ok(cursor.next == 0);
    };
    file.seek(SeekFrom::Start(guard.offset))?;
    let length = usize::try_from(guard.length)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let witness = length.min(RECORD_GUARD_BYTES);
    let mut bytes = vec![0; witness];
    if file.read_exact(&mut bytes).is_err() {
        return Ok(false);
    }
    if length > witness {
        file.seek(SeekFrom::Start(
            guard
                .offset
                .saturating_add(guard.length)
                .saturating_sub(u64::try_from(witness).expect("guard width fits u64")),
        ))?;
        let start = bytes.len();
        bytes.resize(start + witness, 0);
        if file.read_exact(&mut bytes[start..]).is_err() {
            return Ok(false);
        }
    }
    Ok(record_guard_hash_with_length(&bytes, length) == guard.hash)
}

#[cfg(test)]
mod tests;
