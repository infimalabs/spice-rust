use std::fs::{self, OpenOptions};
use std::io::{Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let serial = FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spice-session-compaction-{}-{serial}-{label}",
            std::process::id(),
        ));
        fs::create_dir_all(&path).expect("fixture directory exists");
        Self(path)
    }

    fn transcript(&self) -> PathBuf {
        self.0.join("rollout.jsonl")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Messages(Mutex<Vec<wire::AgentMessage>>);

impl AgentMessagePublisher for Messages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        if let Some(existing) = messages
            .iter()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        {
            return if existing == &message {
                Ok(false)
            } else {
                Err(serve::stream::MessagePublishError::IdentityConflict)
            };
        }
        messages.push(message);
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        let Some(existing) = messages
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(serve::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
}

#[test]
fn preserved_compaction_is_published_once_from_complete_jsonl() {
    let scratch = Scratch::new("published-once");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"message\",\"timestamp\":\"2030-04-12T18:44:08.000Z\",",
            "\"message\":{\"role\":\"assistant\",",
            "\"content\":[{\"type\":\"text\",\"text\":\"before\"}]}}\n",
            "{\"type\":\"compaction\",\"id\":\"c1\",\"timestamp\":\"2030-04-12T18:44:09.271Z\",",
            "\"summary\":\"earlier work\",\"firstKeptEntryId\":\"e7\",\"tokensBefore\":9000}\n",
        ),
    )
    .expect("preserved omp session is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

    let records = scan(&mut cursor).expect("session transcript scans");
    assert_eq!(records.len(), 1, "only the completed boundary is visible");
    assert_eq!(records[0].authored, "2030-04-12T18:44:09.271Z");
    assert!(matches!(records[0].decoded, listen::Event::Compaction(_)));

    let publisher = Messages::default();
    publish(
        records.into_iter().next().expect("one compaction"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        &publisher,
        &mut MessageState::default(),
    )
    .expect("typed boundary publishes");
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert!(!messages[0].authored_synthetic);
    assert!(messages[0].unavailable_tool_terminals.is_empty());
    assert_eq!(
        messages[0].locus.member,
        session_member("018f7777-7777-7444-8888-999999999999"),
        "source chronology does not erase the preserved-session identity",
    );
    assert_eq!(
        messages[0].authored,
        "2030-04-12T18:44:09.271Z"
            .parse::<wire::Instant>()
            .expect("session authored instant parses"),
    );
    assert_eq!(messages[0].observed, None);
    drop(messages);

    assert!(
        scan(&mut cursor)
            .expect("unchanged transcript scans")
            .is_empty(),
        "the cursor does not replay a consumed boundary",
    );
}

#[test]
fn startup_read_volume_depends_on_the_tail_not_older_epochs() {
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T19:44:09.271Z\"}\n";
    let scratch = Scratch::new("bounded-startup");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut older = String::new();
    let mut expected_decoded = None;
    for epochs in [1, 40] {
        for _ in 0..epochs * 100 {
            older.push_str(&assistant_record(0, &filler));
        }
        let mut contents = older.clone();
        contents.push_str(BOUNDARY);
        for index in 0..40 {
            contents.push_str(&assistant_record(index, &filler));
        }
        fs::write(&transcript, &contents).expect("growing session is written");
        let mut cursor = cursor(&transcript, listen::Dialect::Omp);
        cursor.initialized = false;
        BOOTSTRAP_BYTES_DECODED.with(|count| count.set(0));

        let records = bootstrap_until_ready(&mut cursor, "bounded startup scans");

        assert!(
            records.is_empty(),
            "the older boundary is outside the initial 25 cards"
        );
        let decoded = BOOTSTRAP_BYTES_DECODED.with(Cell::get);
        assert_eq!(*expected_decoded.get_or_insert(decoded), decoded);
        assert!(
            decoded <= HISTORY_BYTE_BUDGET as u64,
            "startup decoded {decoded} bytes"
        );
        assert_eq!(cursor.next, contents.len() as u64);
    }
}

#[test]
fn reverse_compaction_pages_cross_one_oversized_record_and_a_partial_tail() {
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T19:44:09.271Z\"}\n";
    let scratch = Scratch::new("oversized-reverse-page");
    let transcript = scratch.transcript();
    let mut contents = BOUNDARY.to_owned();
    contents.push_str(&assistant_record(0, &"x".repeat(100_000)));
    let complete = contents.len() as u64;
    contents.push_str("{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T20:44:09.271Z\"");
    fs::write(&transcript, &contents).expect("partial session is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    cursor.initialized = false;

    let records = bootstrap_until_ready(&mut cursor, "large record makes backward progress");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].locus().offset, 0);
    assert_eq!(cursor.next, complete);
    append(&transcript, b"}\n");
    assert_eq!(
        scan(&mut cursor).expect("partial record completes").len(),
        1
    );
}

#[test]
fn reverse_page_seats_the_cursor_behind_a_terminated_record_larger_than_its_window() {
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T19:44:09.271Z\"}\n";
    let scratch = Scratch::new("terminated-record-over-window");
    let transcript = scratch.transcript();
    let record = assistant_record(0, &"x".repeat(HISTORY_BYTE_BUDGET));
    fs::write(&transcript, &record).expect("oversized record is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    cursor.initialized = false;
    BOOTSTRAP_BYTES_DECODED.with(|count| count.set(0));

    let records = bootstrap_until_ready(&mut cursor, "terminated oversized record is skipped");

    assert!(records.is_empty(), "the skipped record yields no boundary");
    assert_eq!(BOOTSTRAP_BYTES_DECODED.with(Cell::get), 0);
    assert_eq!(
        cursor.next,
        record.len() as u64,
        "the cursor seats on the delimiter the window reached"
    );
    assert_eq!(cursor.prefix_start, cursor.next);
    append(&transcript, BOUNDARY.as_bytes());
    let live = scan_until_boundary(&mut cursor, "boundary appended behind the skipped record");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].locus().offset, record.len() as u64);
}

#[test]
fn reverse_page_refuses_an_unterminated_record_larger_than_its_window() {
    let scratch = Scratch::new("open-record-over-window");
    let transcript = scratch.transcript();
    let record = assistant_record(0, &"x".repeat(HISTORY_BYTE_BUDGET));
    let open = record.trim_end_matches('\n');
    fs::write(&transcript, open).expect("open oversized record is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    cursor.initialized = false;
    BOOTSTRAP_BYTES_DECODED.with(|count| count.set(0));

    let error = bootstrap_until_error(&mut cursor, "open oversized record was admitted");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let text = error.to_string();
    assert!(
        text.contains("runs at least 262144 bytes without a delimiter"),
        "refusal names the record size it crossed: {text}",
    );
    assert!(text.contains("fixed 262144-byte history allowance"));
    let exhaustion = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<HistoryExhaustion>())
        .expect("oversized record refusal retains typed work");
    assert_eq!(exhaustion.work.source_bytes, HISTORY_BYTE_BUDGET as u64);
    assert_eq!(exhaustion.work.decoded_records, 0);
    assert_eq!(exhaustion.work.passes, HISTORY_PASS_BUDGET);
    assert_eq!(BOOTSTRAP_BYTES_DECODED.with(Cell::get), 0);
    assert_eq!(cursor.next, 0, "no record suffix becomes evidence");
    assert!(!cursor.initialized);
    let ScanAttempt::Failed { class, .. } = scan_once_per_source(&mut cursor) else {
        panic!("the open record latches the refusal");
    };
    assert_eq!(class, "history-record-oversized");
    append(&transcript, b"\n");
    let records = (0..128)
        .find_map(|_| match scan_once_per_source(&mut cursor) {
            ScanAttempt::Complete(records) => cursor.initialized.then_some(records),
            ScanAttempt::Latched => panic!("a closed record is not the latched source"),
            ScanAttempt::Failed { error, .. } => panic!("delimiter closes the record: {error}"),
        })
        .expect("bounded authentication settles behind the closed record");
    assert!(
        records.is_empty(),
        "the closed record is skipped rather than decoded"
    );
    assert_eq!(cursor.next, record.len() as u64);
}

#[test]
fn incomplete_boundary_waits_for_its_jsonl_delimiter() {
    let scratch = Scratch::new("partial");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        b"{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.2",
    )
    .expect("partial record is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

    assert!(scan(&mut cursor).expect("partial scan succeeds").is_empty());
    assert_eq!(cursor.next, 0, "pending bytes never advance the cursor");
    append(&transcript, b"71Z\"}\n");
    assert_eq!(scan(&mut cursor).expect("completed scan succeeds").len(), 1,);
}

#[test]
fn mixed_calls_report_only_terminals_unavailable_at_each_boundary() {
    let scratch = Scratch::new("unavailable");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"toolCall\",\"id\":\"call-a\",\"name\":\"bash\",\"arguments\":{\"command\":\"cargo test\"}},",
            "{\"type\":\"toolCall\",\"id\":\"call-b\",\"name\":\"read\",\"arguments\":{\"path\":\"Cargo.toml\"}}]}}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"call-b\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false}}\n",
            "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"toolCall\",\"id\":\"call-c\",\"name\":\"read\",\"arguments\":{\"path\":\"src/lib.rs\"}}]}}\n",
            "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:10.271Z\"}\n",
        ),
    )
    .expect("mixed transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

    let mut records = scan(&mut cursor).expect("first boundary scans");
    records.extend(scan(&mut cursor).expect("second boundary scans"));
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["bash: cargo test"],
        "the concrete Read result wins while the long command remains neutral",
    );
    assert_eq!(
        records[1].unavailable_tool_terminals,
        ["read: src/lib.rs"],
        "the first boundary clears its context epoch",
    );
}

#[test]
fn terminal_result_prevents_an_unavailable_marker() {
    let scratch = Scratch::new("resolved");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"call-a\",\"name\":\"bash\",\"arguments\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"call-a\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false}}\n",
            "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n",
        ),
    )
    .expect("resolved transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

    let records = scan(&mut cursor).expect("resolved transcript scans");
    assert_eq!(records.len(), 1);
    assert!(records[0].unavailable_tool_terminals.is_empty());
}

#[test]
fn typed_image_result_resolves_only_its_own_call_before_compaction() {
    let scratch = Scratch::new("image-resolved");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"call-image\",\"name\":\"read\",\"arguments\":{\"path\":\"capture.png\"}}]}}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"call-command\",\"name\":\"bash\",\"arguments\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"call-image\",\"content\":[{\"type\":\"image\",\"data\":\"iVBORw0KGgo=\",\"mimeType\":\"image/png\"}],\"isError\":false}}\n",
            "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n",
        ),
    )
    .expect("image-result transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

    let records = scan(&mut cursor).expect("image-result transcript scans");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["bash: cargo test"],
        "media bytes neither become a command nor disturb the unrelated pending call",
    );
}

#[test]
fn earlier_same_length_call_replacement_rebuilds_pending_calls() {
    const OLD_CALL: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"old\",\"name\":\"read\",\"arguments\":{\"path\":\"old.rs\"}}]}}\n";
    const NEW_CALL: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"new\",\"name\":\"read\",\"arguments\":{\"path\":\"new.rs\"}}]}}\n";
    const STABLE: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"stable final record\"}]}}\n";
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n";
    assert_eq!(OLD_CALL.len(), NEW_CALL.len());
    let scratch = Scratch::new("call-prefix-replacement");
    let transcript = scratch.transcript();
    let original = format!("{OLD_CALL}{STABLE}");
    let replacement = format!("{NEW_CALL}{STABLE}");
    assert_eq!(original.len(), replacement.len());
    fs::write(&transcript, &original).expect("initial transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    assert!(scan(&mut cursor).expect("initial call scans").is_empty());

    fs::write(&transcript, &replacement).expect("same-length replacement is written");
    assert_eq!(
        fs::metadata(&transcript).expect("metadata").len(),
        cursor.next
    );
    append(&transcript, BOUNDARY.as_bytes());

    let records = scan_until_boundary(&mut cursor, "replacement transcript rescans");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["read: new.rs"]);
    assert_boundary_publishes_once(&transcript, listen::Dialect::Omp, records);
}

#[test]
fn earlier_same_length_result_replacement_rebuilds_pending_calls() {
    const CALL: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"call-a\",\"name\":\"bash\",\"arguments\":{\"command\":\"cargo test\"}}]}}\n";
    const RESULT_A: &str = "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"call-a\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false}}\n";
    const RESULT_B: &str = "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"call-b\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false}}\n";
    const STABLE: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"stable\"}]}}\n";
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n";
    assert_eq!(RESULT_A.len(), RESULT_B.len());
    let scratch = Scratch::new("result-prefix-replacement");
    let transcript = scratch.transcript();
    let original = format!("{CALL}{RESULT_A}{STABLE}");
    let replacement = format!("{CALL}{RESULT_B}{STABLE}");
    assert_eq!(original.len(), replacement.len());
    fs::write(&transcript, &original).expect("resolved transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    assert!(scan(&mut cursor).expect("resolved prefix scans").is_empty());

    fs::write(&transcript, &replacement).expect("same-length result replacement is written");
    assert_eq!(
        fs::metadata(&transcript).expect("metadata").len(),
        cursor.next
    );
    append(&transcript, BOUNDARY.as_bytes());

    let records = scan_until_boundary(&mut cursor, "replacement transcript rescans");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["bash: cargo test"]);
    assert_boundary_publishes_once(&transcript, listen::Dialect::Omp, records);
}

mod authentication;
mod history;

fn assistant_record(index: usize, filler: &str) -> String {
    format!(
        "{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{index} {filler}\"}}]}}}}\n",
    )
}

fn cursor(path: &Path, dialect: listen::Dialect) -> Cursor {
    Cursor {
        thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
        path: path.to_path_buf(),
        dialect,
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        member: session_member("018f7777-7777-7444-8888-999999999999"),
        initialized: true,
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

fn bootstrap_until_ready(cursor: &mut Cursor, context: &str) -> Vec<CompactionRecord> {
    let mut records = Vec::new();
    for _ in 0..128 {
        records.extend(scan(cursor).unwrap_or_else(|error| panic!("{context}: {error}")));
        if cursor.initialized {
            return records;
        }
    }
    panic!("{context}: bounded authentication did not settle")
}

fn bootstrap_until_error(cursor: &mut Cursor, context: &str) -> io::Error {
    for _ in 0..128 {
        match scan(cursor) {
            Ok(_) => {}
            Err(error) => return error,
        }
    }
    panic!("{context}: expected a bounded bootstrap refusal")
}

fn append(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .expect("transcript opens")
        .write_all(bytes)
        .expect("transcript append succeeds");
}

fn assert_boundary_publishes_once(
    transcript: &Path,
    dialect: listen::Dialect,
    records: Vec<CompactionRecord>,
) {
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let author = uuid("018f82d4-6f59-7d06-92e3-bce9131163ef");
    let publisher = Messages::default();
    let mut state = MessageState::default();
    for record in records {
        publish(record, lane, author, &publisher, &mut state).expect("boundary publishes");
    }

    let mut reloaded = cursor(transcript, dialect);
    let replay = scan(&mut reloaded).expect("resident reload rescans canonical bytes");
    assert_eq!(replay.len(), 1, "reload finds the one durable boundary");
    assert!(
        scan(&mut reloaded)
            .expect("unchanged poll scans")
            .is_empty()
    );
    for record in replay {
        publish(record, lane, author, &publisher, &mut state)
            .expect("boundary replay deduplicates");
    }
    assert_eq!(
        publisher.0.lock().expect("message lock").len(),
        1,
        "poll and reload retain one published boundary",
    );
}

fn scan_until_boundary(cursor: &mut Cursor, label: &str) -> Vec<CompactionRecord> {
    for _ in 0..16 {
        let records = scan(cursor).unwrap_or_else(|error| panic!("{label}: {error}"));
        if !records.is_empty() {
            return records;
        }
    }
    panic!("{label}: bounded replay did not reach the boundary");
}

fn large_transcript(fill: char) -> String {
    let text = fill.to_string().repeat(4 * 1024);
    let record = format!(
        "{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n",
    );
    format!(
        "{}{{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}}\n",
        record.repeat(96),
    )
}

fn large_transcript_with_pending(fill: char, records: usize) -> String {
    const PENDING_CALL: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"held\",\"name\":\"read\",\"arguments\":{\"path\":\"held.rs\"}}]}}\n";
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n";
    let text = fill.to_string().repeat(4 * 1024);
    let record = format!(
        "{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n",
    );
    format!("{}{PENDING_CALL}{BOUNDARY}", record.repeat(records))
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}
