//! Structured compaction boundaries recovered from preserved vendor sessions.

#[cfg(test)]
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

use super::*;

mod file_stamp;
mod history_authentication;
mod identity;
mod presentation;
use file_stamp::*;
#[cfg(test)]
use history_authentication::AFTER_HISTORY_AUTHENTICATION_READ;
pub(super) use history_authentication::{HistoryCheckpoint, history_checkpoint};
use history_authentication::{advance_history_authentication, observe_history_source_movement};
pub(super) use identity::session_member;
use presentation::publish;

/// Per-resident cursors over canonical session JSONL files.
///
/// Codex's `exec --json` stream does not expose the `compacted` record stored
/// in its rollout, so selected-stream capture cannot manufacture this view.
/// The preserved transcript is already the durable source used by `session`;
/// this reader records Codex boundaries into lane authority, projects typed
/// compaction events into Canvas, and folds adjacent tool evidence only to
/// describe terminal records missing at each boundary.
pub(in crate::resident) struct SessionCompactions {
    environment: crate::session_command::Environment,
    cursors: BTreeMap<CanonicalUuid, Cursor>,
    reported: BTreeMap<CanonicalUuid, (Option<PathBuf>, String)>,
}

struct Cursor {
    thread: String,
    path: PathBuf,
    worktree: PathBuf,
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
    startup_recovery: Option<listen::Locus>,
    startup_search: Option<HistoryCheckpoint>,
}

struct PendingTool {
    order: u64,
    preview: String,
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
const HISTORY_BYTE_BUDGET: usize = 256 * 1024;
const HISTORY_RECORD_BUDGET: usize = 128;
const HISTORY_PASS_BUDGET: u32 = 1;
const HISTORY_WALL_BUDGET: Duration = Duration::from_millis(50);

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
    static AFTER_HISTORY_SOURCE_READ: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
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
        bus: &Bus,
        epoch: CanonicalUuid,
        selection: (&wire::ResidencyHead, CanonicalUuid),
        publisher: &impl AgentMessagePublisher,
        state: &mut MessageState,
        diagnostic: &mut dyn io::Write,
    ) -> CommandResult<()> {
        let (residency, lane) = selection;
        self.synchronize_selected_with(
            residency,
            Some(lane),
            publisher,
            state,
            diagnostic,
            |lane, occupant, thread, locus| {
                lane_surface::recover_session_compaction(bus, epoch, lane, occupant, thread, locus)
                    .map(drop)
            },
        )
    }

    #[cfg(test)]
    fn synchronize_with<F>(
        &mut self,
        residency: &wire::ResidencyHead,
        publisher: &impl AgentMessagePublisher,
        state: &mut MessageState,
        diagnostic: &mut dyn io::Write,
        recover: F,
    ) -> CommandResult<()>
    where
        F: FnMut(CanonicalUuid, CanonicalUuid, &str, listen::Locus) -> CommandResult<()>,
    {
        self.synchronize_selected_with(residency, None, publisher, state, diagnostic, recover)
    }

    fn synchronize_selected_with<F>(
        &mut self,
        residency: &wire::ResidencyHead,
        selected: Option<CanonicalUuid>,
        publisher: &impl AgentMessagePublisher,
        state: &mut MessageState,
        diagnostic: &mut dyn io::Write,
        mut recover: F,
    ) -> CommandResult<()>
    where
        F: FnMut(CanonicalUuid, CanonicalUuid, &str, listen::Locus) -> CommandResult<()>,
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
            let Some((scanned, startup_recovery, path, worktree, author, dialect, thread)) =
                self.cursors.get_mut(&lane).map(|cursor| {
                    let path = cursor.path.clone();
                    let worktree = cursor.worktree.clone();
                    let author = cursor.author;
                    let dialect = cursor.dialect;
                    let thread = cursor.thread.clone();
                    let scanned = scan(cursor);
                    let startup_recovery = cursor.startup_recovery.take();
                    (
                        scanned,
                        startup_recovery,
                        path,
                        worktree,
                        author,
                        dialect,
                        thread,
                    )
                })
            else {
                continue;
            };
            match scanned {
                Ok(records) => {
                    if dialect == listen::Dialect::Codex
                        && let Some(locus) = startup_recovery
                    {
                        recover(lane, author, &thread, locus)?;
                    }
                    for record in records {
                        if dialect == listen::Dialect::Codex && startup_recovery.is_none() {
                            recover(lane, author, &thread, record.locus())?;
                        }
                        publish(record, lane, author, &worktree, publisher, state)?;
                    }
                }
                Err(error) => self.report_scan_once(lane, &path, error, diagnostic),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn synchronize_display_only(
        &mut self,
        residency: &wire::ResidencyHead,
        publisher: &impl AgentMessagePublisher,
        state: &mut MessageState,
        diagnostic: &mut dyn io::Write,
    ) -> CommandResult<()> {
        self.synchronize_with(residency, publisher, state, diagnostic, |_, _, _, _| Ok(()))
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
            &self.environment,
        ) {
            Ok((path, dialect)) => {
                self.reported.remove(&lane);
                self.cursors.insert(
                    lane,
                    Cursor::new(
                        session.identity.clone(),
                        path,
                        session.worktree.clone().into(),
                        dialect,
                        session.occupant,
                    ),
                );
            }
            Err(error) => self.report_once(lane, (None, error.clone()), error, diagnostic),
        }
    }

    fn report_scan_once(
        &mut self,
        lane: CanonicalUuid,
        path: &PathBuf,
        error: io::Error,
        diagnostic: &mut dyn io::Write,
    ) {
        let class = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<HistoryExhaustion>())
            .map_or_else(
                || format!("{:?}: {error}", error.kind()),
                |error| {
                    if error.oversized_record {
                        "history-record-oversized".to_owned()
                    } else {
                        "history-work-exhausted".to_owned()
                    }
                },
            );
        self.report_once(
            lane,
            (Some(path.clone()), class),
            format!("{}: {error}", path.display()),
            diagnostic,
        );
    }

    fn report_once(
        &mut self,
        lane: CanonicalUuid,
        key: (Option<PathBuf>, String),
        detail: String,
        diagnostic: &mut dyn io::Write,
    ) {
        if self.reported.get(&lane) == Some(&key) {
            return;
        }
        self.reported.insert(lane, key);
        let _ = writeln!(
            diagnostic,
            "canvas session compaction unavailable for lane {lane}: {detail}",
        );
    }
}

impl Cursor {
    fn new(
        thread: String,
        path: PathBuf,
        worktree: PathBuf,
        dialect: listen::Dialect,
        author: CanonicalUuid,
    ) -> Self {
        Self {
            member: session_member(&thread),
            thread,
            path,
            worktree,
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
            startup_recovery: None,
            startup_search: None,
        }
    }
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
        if cursor.dialect == listen::Dialect::Codex {
            return bootstrap_codex(cursor, canvas::stream::INITIAL_MESSAGE_LIMIT + 1);
        }
        return bootstrap(cursor, canvas::stream::INITIAL_MESSAGE_LIMIT + 1);
    }
    if cursor.validation.is_some() {
        return advance_validation(cursor);
    }
    if cursor.dialect == listen::Dialect::Codex
        && cursor.startup_search.is_some()
        && File::open(&cursor.path)?.metadata()?.len() == cursor.next
    {
        return advance_codex_startup_search(cursor);
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
    if cursor.dialect == listen::Dialect::Codex {
        cursor.startup_search = None;
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

/// Find the latest durable Codex boundary by paging backward, independently
/// of the visible Canvas tail. Older context epochs cannot change that boundary.
fn bootstrap_codex(cursor: &mut Cursor, visible_limit: usize) -> io::Result<Vec<CompactionRecord>> {
    let checkpoint = cursor
        .startup_search
        .clone()
        .map_or_else(|| history_checkpoint(&cursor.path), Ok)?;
    let page = bootstrap_before(cursor, visible_limit, &checkpoint)?;
    if page.pending_authentication {
        cursor.startup_search = Some(checkpoint);
        return Ok(Vec::new());
    }
    let latest = page.latest;
    cursor.startup_recovery = latest;
    cursor.startup_search =
        (latest.is_none() && page.start > 0).then(|| checkpoint.retreat(page.start));
    Ok(page.records)
}

fn advance_codex_startup_search(cursor: &mut Cursor) -> io::Result<Vec<CompactionRecord>> {
    let checkpoint = cursor
        .startup_search
        .clone()
        .expect("Codex startup search advances only while pending");
    let mut recovery = Cursor::new(
        cursor.thread.clone(),
        cursor.path.clone(),
        cursor.worktree.clone(),
        cursor.dialect,
        cursor.author,
    );
    let page = bootstrap_before(&mut recovery, 0, &checkpoint)?;
    if page.pending_authentication {
        return Ok(Vec::new());
    }
    if let Some(boundary) = page.latest {
        cursor.startup_recovery = Some(boundary);
        cursor.startup_search = None;
    } else if page.start == 0 {
        cursor.startup_search = None;
    } else {
        cursor.startup_search = Some(checkpoint.retreat(page.start));
    }
    Ok(Vec::new())
}

/// Seed a new resident from the bounded initial visible tail Canvas presents.
///
/// A forward replay from byte zero makes the hundredth boundary pay for the
/// first ninety-nine prefixes again. Read one fixed newest-record page, retain
/// its older continuation, and continue live scanning from that snapshot.
fn bootstrap(cursor: &mut Cursor, visible_limit: usize) -> io::Result<Vec<CompactionRecord>> {
    let checkpoint = cursor
        .startup_search
        .clone()
        .map_or_else(|| history_checkpoint(&cursor.path), Ok)?;
    let page = bootstrap_before(cursor, visible_limit, &checkpoint)?;
    if page.pending_authentication {
        cursor.startup_search = Some(checkpoint);
        return Ok(Vec::new());
    }
    cursor.startup_search = None;
    Ok(page.records)
}

struct HistoryPage {
    records: Vec<CompactionRecord>,
    latest: Option<listen::Locus>,
    start: u64,
    work: HistoryWork,
    pending_authentication: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HistoryWork {
    pub(super) source_bytes: u64,
    pub(super) decoded_records: u32,
    pub(super) passes: u32,
    pub(super) elapsed: Duration,
}

impl HistoryWork {
    pub(super) fn within_limits(self) -> bool {
        self.source_bytes <= HISTORY_BYTE_BUDGET as u64
            && self.decoded_records <= HISTORY_RECORD_BUDGET as u32
            && self.passes <= HISTORY_PASS_BUDGET
            && self.elapsed <= HISTORY_WALL_BUDGET
    }

    fn sampled(mut self, started: Instant) -> Self {
        self.elapsed = started.elapsed();
        self
    }
}

fn bootstrap_before(
    cursor: &mut Cursor,
    visible_limit: usize,
    checkpoint: &HistoryCheckpoint,
) -> io::Result<HistoryPage> {
    let started = Instant::now();
    let authentication = advance_history_authentication(&cursor.path, checkpoint, started)?;
    let Some(stamp) = authentication.stamp else {
        return Ok(HistoryPage {
            records: Vec::new(),
            latest: None,
            start: checkpoint.next(),
            work: authentication.work,
            pending_authentication: true,
        });
    };
    let Some(source) = read_history_source(&cursor.path, checkpoint, stamp, started)? else {
        return Ok(HistoryPage {
            records: Vec::new(),
            latest: None,
            start: checkpoint.next(),
            work: authentication.work,
            pending_authentication: true,
        });
    };
    let selected = selected_history_start(&source.bytes);
    let start = source.aligned.saturating_add(selected as u64);
    prepare_history_cursor(cursor, start);
    let mut work = source.work;
    let (records, latest) = decode_history_records(
        cursor,
        &source.bytes[selected..],
        visible_limit,
        source.stamp,
        started,
        &mut work,
    )?;
    let observed = FileStamp::read(&File::open(&cursor.path)?)?;
    if observed != source.stamp {
        observe_history_source_movement(checkpoint, observed)?;
        reset(cursor);
        return Ok(HistoryPage {
            records: Vec::new(),
            latest: None,
            start,
            work,
            pending_authentication: true,
        });
    }
    work.elapsed = started.elapsed();
    if !work.within_limits() {
        return Err(history_budget_exhausted(work));
    }
    cursor.initialized = true;
    cursor.prefix_start = cursor.next;
    cursor.prefix = blake3::Hasher::new();
    Ok(HistoryPage {
        records,
        latest,
        start,
        work,
        pending_authentication: false,
    })
}

struct HistorySource {
    stamp: FileStamp,
    aligned: u64,
    bytes: Vec<u8>,
    work: HistoryWork,
}

fn read_history_source(
    path: &PathBuf,
    checkpoint: &HistoryCheckpoint,
    authenticated: FileStamp,
    started: Instant,
) -> io::Result<Option<HistorySource>> {
    let mut file = File::open(path)?;
    let stamp = FileStamp::read(&file)?;
    if stamp != authenticated {
        observe_history_source_movement(checkpoint, stamp)?;
        return Ok(None);
    }
    let end = checkpoint.next();
    let source_bytes = end.min(HISTORY_BYTE_BUDGET as u64);
    let source_start = end.saturating_sub(source_bytes);
    file.seek(SeekFrom::Start(source_start))?;
    let mut bytes = vec![0; usize::try_from(source_bytes).expect("history budget fits usize")];
    file.read_exact(&mut bytes)?;
    #[cfg(test)]
    AFTER_HISTORY_SOURCE_READ.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let work = HistoryWork {
        source_bytes,
        decoded_records: 0,
        passes: 1,
        elapsed: started.elapsed(),
    };
    if !work.within_limits() {
        return Err(history_budget_exhausted(work));
    }
    let aligned = align_history_source(&mut bytes, source_start, end, work, started)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1);
    bytes.truncate(complete);
    Ok(Some(HistorySource {
        stamp,
        aligned,
        bytes,
        work,
    }))
}

fn align_history_source(
    bytes: &mut Vec<u8>,
    source_start: u64,
    end: u64,
    work: HistoryWork,
    started: Instant,
) -> io::Result<u64> {
    if source_start == 0 {
        return Ok(0);
    }
    let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
        return Err(history_record_exceeds_budget(work.sampled(started)));
    };
    let aligned = source_start.saturating_add(newline as u64 + 1);
    if aligned == end {
        return Err(history_record_exceeds_budget(work.sampled(started)));
    }
    bytes.drain(..=newline);
    Ok(aligned)
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
) -> io::Result<(Vec<CompactionRecord>, Option<listen::Locus>)> {
    let mut records = Vec::new();
    let mut visible = Vec::new();
    for record in bytes.split_inclusive(|byte| *byte == b'\n') {
        *work = work.sampled(started);
        if !work.within_limits() {
            return Err(history_budget_exhausted(*work));
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
    let latest = records.last().map(CompactionRecord::locus);
    retain_visible_history(&mut records, &visible, visible_limit, cursor.next);
    Ok((records, latest))
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
                "preserved session record exceeds the fixed {HISTORY_BYTE_BUDGET}-byte history allowance after {} bytes, {} records, {} passes, and {:?}",
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

pub(super) struct HistoryScan {
    pub(super) messages: Vec<wire::AgentMessage>,
    pub(super) more: bool,
    pub(super) start: u64,
    pub(super) work: HistoryWork,
    pub(super) pending_authentication: bool,
}

pub(super) struct HistoryRequest<'a> {
    pub(super) path: PathBuf,
    pub(super) worktree: PathBuf,
    pub(super) dialect: listen::Dialect,
    pub(super) thread: String,
    pub(super) lane: CanonicalUuid,
    pub(super) author: CanonicalUuid,
    pub(super) observations: &'a MessageObservations,
    pub(super) before: &'a wire::AgentMessage,
    pub(super) end: &'a HistoryCheckpoint,
    pub(super) limit: usize,
}

pub(super) fn retained_messages(request: HistoryRequest<'_>) -> CommandResult<HistoryScan> {
    let mut cursor = Cursor::new(
        request.thread,
        request.path,
        request.worktree.clone(),
        request.dialect,
        request.author,
    );
    let page = bootstrap_before(&mut cursor, 0, request.end)
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    let messages = HistoryMessages::new(request.limit);
    let mut state = MessageState {
        previews: CallPreviews::default(),
        observations: request.observations.clone(),
        prose: BTreeMap::new(),
    };
    for record in page.records {
        publish(
            record,
            request.lane,
            request.author,
            &request.worktree,
            &messages,
            &mut state,
        )?;
    }
    let collected = messages.inner.into_inner().map_err(|_| {
        store_fault(
            "canvas::message-history-unavailable",
            "history lock poisoned",
        )
    })?;
    let mut messages = collected.messages;
    messages.retain(|message| message.chronology_key() < request.before.chronology_key());
    let start = messages.len().saturating_sub(request.limit);
    let selected = messages.split_off(start);
    Ok(HistoryScan {
        messages: selected,
        more: collected.dropped || start > 0 || page.start > 0,
        start: page.start,
        work: page.work,
        pending_authentication: page.pending_authentication,
    })
}

struct HistoryMessages {
    limit: usize,
    inner: std::sync::Mutex<HistoryCollection>,
}

#[derive(Default)]
struct HistoryCollection {
    messages: Vec<wire::AgentMessage>,
    dropped: bool,
}

impl HistoryMessages {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            inner: std::sync::Mutex::new(HistoryCollection::default()),
        }
    }
}

impl AgentMessagePublisher for HistoryMessages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut collected = self
            .inner
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?;
        collected.messages.push(message);
        if collected.messages.len() > self.limit {
            collected.messages.remove(0);
            collected.dropped = true;
        }
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut collected = self
            .inner
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?;
        let Some(existing) = collected
            .messages
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(canvas::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
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
    let decoded = listen::decode(cursor.dialect, bytes, locus)
        .expect("the locus is built from the exact session record bytes");
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
        listen::Event::Prose(prose) => prose.role == listen::ProseRole::Assistant,
        listen::Event::Compaction(_) => true,
        listen::Event::Tool(tool) => match &tool.moment {
            listen::ToolMoment::Result(outcome) => !outcome.media.is_empty(),
            listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => false,
        },
        _ => false,
    }
}

fn fold_tool(cursor: &mut Cursor, tool: &listen::Tool) {
    match &tool.moment {
        listen::ToolMoment::Call { name, arguments }
        | listen::ToolMoment::Update { name, arguments } => {
            let preview = super::tool_preview::call_preview(name, arguments);
            if let Some(pending) = cursor.pending.get_mut(&tool.call_id) {
                pending.preview = preview;
            } else {
                let order = cursor.next_call;
                cursor.next_call = cursor.next_call.saturating_add(1);
                cursor
                    .pending
                    .insert(tool.call_id.clone(), PendingTool { order, preview });
            }
        }
        listen::ToolMoment::Result(_) => {
            cursor.pending.remove(&tool.call_id);
        }
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
    cursor.startup_recovery = None;
    cursor.startup_search = None;
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
