//! Structured compaction boundaries recovered from preserved vendor sessions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;

use super::*;

/// Polling is intentionally modest: compaction is rare, while the first
/// record after it must still place the divider before later authored prose.
pub(in crate::resident) const POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(500);

/// Per-resident cursors over canonical session JSONL files.
///
/// Codex's `exec --json` stream does not expose the `compacted` record stored
/// in its rollout, so selected-stream capture cannot manufacture this view.
/// The preserved transcript is already the durable source used by `session`;
/// this reader projects typed compaction events into Canvas and folds adjacent
/// tool evidence only to describe terminal records missing at each boundary.
pub(in crate::resident) struct SessionCompactions {
    environment: crate::session_command::Environment,
    cursors: BTreeMap<CanonicalUuid, Cursor>,
    reported: BTreeMap<CanonicalUuid, String>,
}

struct Cursor {
    thread: String,
    path: PathBuf,
    dialect: listen::Dialect,
    author: CanonicalUuid,
    member: u64,
    next: u64,
    next_call: u64,
    pending: BTreeMap<String, PendingTool>,
    guard: Option<RecordGuard>,
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

impl SessionCompactions {
    pub(in crate::resident) fn new(environment: crate::session_command::Environment) -> Self {
        Self {
            environment,
            cursors: BTreeMap::new(),
            reported: BTreeMap::new(),
        }
    }

    /// Publish every newly completed structured boundary for current sessions.
    pub(in crate::resident) fn synchronize(
        &mut self,
        residency: &wire::ResidencyHead,
        publisher: &impl AgentMessagePublisher,
        state: &mut MessageState,
        diagnostic: &mut dyn io::Write,
    ) -> CommandResult<()> {
        let current = residency
            .lanes
            .iter()
            .filter_map(|lane| lane.session.as_ref().map(|session| (lane.lane, session)))
            .collect::<BTreeMap<_, _>>();
        let lanes = current.keys().copied().collect::<BTreeSet<_>>();
        self.cursors.retain(|lane, _| lanes.contains(lane));
        self.reported.retain(|lane, _| lanes.contains(lane));

        for (lane, session) in current {
            self.ensure_cursor(lane, session, diagnostic);
            let Some((scanned, path, author)) = self.cursors.get_mut(&lane).map(|cursor| {
                let path = cursor.path.clone();
                let author = cursor.author;
                (scan(cursor), path, author)
            }) else {
                continue;
            };
            match scanned {
                Ok(records) => {
                    self.reported.remove(&lane);
                    for record in records {
                        publish(record, lane, author, publisher, state)?;
                    }
                }
                Err(error) => {
                    self.report_once(lane, format!("{}: {error}", path.display()), diagnostic)
                }
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
            &self.environment,
        ) {
            Ok((path, dialect)) => {
                self.reported.remove(&lane);
                self.cursors.insert(
                    lane,
                    Cursor {
                        thread: session.identity.clone(),
                        path,
                        dialect,
                        author: session.occupant,
                        member: session_member(&session.identity),
                        next: 0,
                        next_call: 0,
                        pending: BTreeMap::new(),
                        guard: None,
                    },
                );
            }
            Err(error) => self.report_once(lane, error, diagnostic),
        }
    }

    fn report_once(&mut self, lane: CanonicalUuid, detail: String, diagnostic: &mut dyn io::Write) {
        if self.reported.get(&lane) == Some(&detail) {
            return;
        }
        self.reported.insert(lane, detail.clone());
        let _ = writeln!(
            diagnostic,
            "canvas session compaction unavailable for lane {lane}: {detail}",
        );
    }
}

struct CompactionRecord {
    decoded: listen::Event,
    authored: Option<String>,
    ordinal: u32,
    unavailable_tool_terminals: Vec<String>,
}

fn scan(cursor: &mut Cursor) -> io::Result<Vec<CompactionRecord>> {
    let metadata = fs::metadata(&cursor.path)?;
    if metadata.len() < cursor.next || !guard_matches(cursor)? {
        reset(cursor);
    }
    let mut file = File::open(&cursor.path)?;
    file.seek(SeekFrom::Start(cursor.next))?;
    let mut reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 || bytes.last() != Some(&b'\n') {
            break;
        }
        let length = u64::try_from(read).expect("a record buffer length fits u64");
        let locus = listen::Locus {
            member: cursor.member,
            offset: cursor.next,
            length,
        };
        let offset = cursor.next;
        cursor.next = cursor.next.saturating_add(length);
        cursor.guard = Some(RecordGuard {
            offset,
            length,
            hash: record_guard_hash(&bytes),
        });
        let decoded = listen::decode(cursor.dialect, &bytes, locus)
            .expect("the locus is built from the exact session record bytes");
        let authored = decoded
            .context()
            .and_then(|context| context.timestamp.clone());
        for (ordinal, event) in decoded.events().iter().enumerate() {
            let ordinal = u32::try_from(ordinal)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
            match event {
                listen::Event::Tool(tool) => fold_tool(cursor, tool),
                listen::Event::Compaction(_) => {
                    let mut unavailable = cursor.pending.values().collect::<Vec<_>>();
                    unavailable.sort_by_key(|tool| tool.order);
                    records.push(CompactionRecord {
                        decoded: event.clone(),
                        authored: authored.clone(),
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
    }
    Ok(records)
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
    cursor.next = 0;
    cursor.next_call = 0;
    cursor.pending.clear();
    cursor.guard = None;
}

fn guard_matches(cursor: &Cursor) -> io::Result<bool> {
    let Some(guard) = &cursor.guard else {
        return Ok(true);
    };
    let mut file = File::open(&cursor.path)?;
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

fn record_guard_hash(bytes: &[u8]) -> u64 {
    let witness = bytes.len().min(RECORD_GUARD_BYTES);
    let hash = hash_bytes(0xcbf2_9ce4_8422_2325, &bytes.len().to_le_bytes());
    let hash = hash_bytes(hash, &bytes[..witness]);
    if bytes.len() > witness {
        hash_bytes(hash, &bytes[bytes.len() - witness..])
    } else {
        hash
    }
}

fn record_guard_hash_with_length(bytes: &[u8], length: usize) -> u64 {
    let hash = hash_bytes(0xcbf2_9ce4_8422_2325, &length.to_le_bytes());
    hash_bytes(hash, bytes)
}

fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn publish(
    record: CompactionRecord,
    lane: CanonicalUuid,
    author: CanonicalUuid,
    publisher: &impl AgentMessagePublisher,
    state: &mut MessageState,
) -> CommandResult<()> {
    publish_agent_event(
        publisher,
        AgentEvent {
            lane,
            author,
            source: None,
            ordinal: record.ordinal,
            decoded: &record.decoded,
            authored: record.authored.as_deref(),
            observed: state.observations.latest(lane),
            observations: &state.observations,
            unavailable_tool_terminals: &record.unavailable_tool_terminals,
        },
        &mut state.previews,
        None,
    )
}

/// Namespace preserved-session loci away from tape member ordinals while
/// retaining a deterministic identity across resident and browser restarts.
fn session_member(thread: &str) -> u64 {
    let mut value = 0_u64;
    let mut digits = 0_u32;
    for byte in thread.bytes().filter(u8::is_ascii_hexdigit) {
        let nibble = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            b'A'..=b'F' => u64::from(byte - b'A' + 10),
            _ => unreachable!("the filter admits only hexadecimal digits"),
        };
        value = (value << 4) | nibble;
        digits += 1;
        if digits == 15 {
            break;
        }
    }
    SESSION_MESSAGE_MEMBER_NAMESPACE | value
}

#[cfg(test)]
mod tests;
