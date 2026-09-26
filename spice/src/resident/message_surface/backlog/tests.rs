use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// Tool records outgrowing one startup window at near 1.9 KiB each.
const SPARSE_TOOL_RECORDS: usize = 640;
/// Tool records outgrowing one history window at near 1.9 KiB each.
const SPARSE_HORIZON_RECORDS: usize = 4_600;

fn visible_cards(scan: &listen::Scan) -> usize {
    scan.records
        .iter()
        .map(|record| {
            let turn = usize::from(
                record.context.as_ref().and_then(|context| context.turn)
                    == Some(listen::TurnMoment::Started),
            );
            let listen::RecordPayload::Events(events) = &record.payload else {
                return turn;
            };
            turn + events
                .iter()
                .filter(|event| match &event.event {
                    listen::Event::Prose(prose) => prose.role == listen::ProseRole::Assistant,
                    listen::Event::Compaction(_) => true,
                    listen::Event::Tool(tool) => match &tool.moment {
                        listen::ToolMoment::Result(outcome) => !outcome.media.is_empty(),
                        listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => {
                            false
                        }
                    },
                    _ => false,
                })
                .count()
        })
        .sum()
}

#[test]
fn startup_work_caps_lane_reads_and_total_source_bytes() {
    let mut aggregate = StartupWork::default();
    for _ in 0..STARTUP_LANE_READ_LIMIT {
        assert!(aggregate.admits_lane(WIDEST_DIALECT));
        aggregate.begin_lane();
        aggregate.add(listen::TailWork {
            source_bytes: startup_source_bytes(WIDEST_DIALECT),
            records: 1,
            passes: 1,
            duration: Duration::from_millis(1),
        });
    }
    assert!(
        !aggregate.admits_lane(listen::Dialect::Codex),
        "the spent ceiling closes startup to a short-record dialect too",
    );
    assert_eq!(aggregate.lanes, STARTUP_LANE_READ_LIMIT);
    assert_eq!(aggregate.source_bytes, STARTUP_SOURCE_BYTE_LIMIT);
    assert_eq!(aggregate.records, STARTUP_LANE_READ_LIMIT);
    assert_eq!(aggregate.passes, STARTUP_LANE_READ_LIMIT);
}

#[test]
fn replay_uses_the_worktree_bound_to_each_session_locus() {
    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e5"
        .parse()
        .expect("fixture lane");
    let source: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e6"
        .parse()
        .expect("fixture source");
    let worktrees = SessionWorktrees(BTreeMap::from([(
        (lane, source),
        vec![
            SessionWorktree {
                member: 1,
                offset: 10,
                worktree: PathBuf::from("/worktrees/old"),
            },
            SessionWorktree {
                member: 1,
                offset: 30,
                worktree: PathBuf::from("/worktrees/new"),
            },
        ],
    )]));

    assert_eq!(
        worktrees.at(lane, source, tape::Locus::new(1, 20, 1)),
        Some(Path::new("/worktrees/old")),
    );
    assert_eq!(
        worktrees.at(lane, source, tape::Locus::new(1, 40, 1)),
        Some(Path::new("/worktrees/new")),
    );
}

#[test]
fn codex_completed_turn_materializes_history_as_one_final_card() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-final-history-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("final history tape opens");
    tape.tee(
        concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"final\",",
            "\"type\":\"agent_message\",\"text\":\"settled final\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,",
            "\"cached_input_tokens\":0,\"output_tokens\":1}}\n",
        )
        .as_bytes(),
    )
    .expect("final history is durable");
    let scan = listen::scan(&root, listen::Dialect::Codex, None).expect("final history scans");
    let messages = CollectedMessages::new(canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT);
    let mut reader = BacklogReader::default();
    let mut diagnostic = Vec::new();
    publish_scan(
        scan,
        "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("fixture lane"),
        "018f82d4-6f59-7d06-92e3-bce9131163ef"
            .parse()
            .expect("fixture author"),
        Path::new("/fixture-worktree"),
        &messages,
        &mut reader,
        &mut diagnostic,
    )
    .expect("final history materializes");

    assert!(diagnostic.is_empty());
    let collected = messages.inner.into_inner().expect("history lock").messages;
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].text, "settled final");
    assert!(collected[0].final_answer);
    fs::remove_dir_all(root).expect("final history fixture cleanup");
}

#[test]
fn replay_keeps_vendor_turn_boundaries_after_compaction() {
    assert_replay_boundary(
        "codex",
        listen::Dialect::Codex,
        concat!(
            "{\"timestamp\":\"2026-08-09T01:00:00.000Z\",\"type\":\"compacted\",",
            "\"payload\":{}}\n",
            "{\"timestamp\":\"2026-08-09T01:00:01.000Z\",\"type\":\"event_msg\",",
            "\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-2\"}}\n",
            "{\"timestamp\":\"2026-08-09T01:00:02.000Z\",\"type\":\"item.completed\",",
            "\"item\":{\"id\":\"commentary\",\"type\":\"agent_message\",",
            "\"text\":\"resumed work\"}}\n",
        ),
    );
    assert_replay_boundary(
        "claude",
        listen::Dialect::Claude,
        concat!(
            "{\"timestamp\":\"2026-08-09T01:00:00.000Z\",\"type\":\"summary\",",
            "\"summary\":\"compacted context\"}\n",
            "{\"timestamp\":\"2026-08-09T01:00:01.000Z\",\"type\":\"user\",",
            "\"promptId\":\"turn-2\",\"message\":{\"type\":\"message\",\"role\":\"user\",",
            "\"content\":[{\"type\":\"text\",\"text\":\"continue\"}]}}\n",
            "{\"timestamp\":\"2026-08-09T01:00:02.000Z\",\"type\":\"assistant\",",
            "\"message\":{\"type\":\"message\",\"role\":\"assistant\",",
            "\"content\":[{\"type\":\"text\",\"text\":\"resumed work\"}]}}\n",
        ),
    );
}

fn assert_replay_boundary(label: &str, dialect: listen::Dialect, contents: &str) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-turn-boundary-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("turn boundary tape opens");
    tape.tee(contents.as_bytes())
        .expect("compacted turn sequence is durable");
    let scan = listen::scan(&root, dialect, None).expect("turn sequence scans");
    let messages = CollectedMessages::new(canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT);
    let mut reader = BacklogReader::default();
    let mut diagnostic = Vec::new();
    publish_scan(
        scan,
        "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("fixture lane"),
        "018f82d4-6f59-7d06-92e3-bce9131163ef"
            .parse()
            .expect("fixture author"),
        Path::new("/fixture-worktree"),
        &messages,
        &mut reader,
        &mut diagnostic,
    )
    .expect("compacted turn sequence materializes");

    assert!(diagnostic.is_empty());
    let collected = messages.inner.into_inner().expect("history lock").messages;
    assert_eq!(
        collected
            .iter()
            .map(|message| message.activity)
            .collect::<Vec<_>>(),
        vec![
            wire::AgentActivityKind::Compaction,
            wire::AgentActivityKind::Turn,
            wire::AgentActivityKind::Prose,
        ]
    );
    assert_eq!(collected[1].text, "New turn");
    assert_eq!(collected[2].text, "resumed work");
    fs::remove_dir_all(root).expect("turn boundary fixture cleanup");
}

fn timestamp(index: usize) -> String {
    format!(
        "2030-04-12T18:{:02}:{:02}.000Z",
        40 + index / 60,
        index % 60,
    )
}

struct MergedHistoryFixture {
    root: PathBuf,
    session_path: PathBuf,
    session_contents: String,
    lane: CanonicalUuid,
    source_end: listen::Checkpoint,
    history: DurableHistory,
    future: wire::AgentMessage,
}

fn merged_history_fixture() -> MergedHistoryFixture {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-merged-history-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape_root = root.join("tape");
    let session_path = root.join("rollout.jsonl");
    fs::create_dir_all(&root).expect("merged fixture root exists");
    let tape = tape::Tape::open(&tape_root).expect("merged tape opens");
    let filler = "x".repeat(1_800);
    let mut tape_contents = Vec::new();
    let mut session_contents = String::new();
    for index in 0..140 {
        let timestamp = timestamp(index);
        tape_contents.extend_from_slice(
            format!(
                "{{\"timestamp\":\"{timestamp}\",\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"message {index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
        session_contents.push_str(&format!(
            "{{\"timestamp\":\"{timestamp}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"message {index}\"}}]}}}}\n"
        ));
        if matches!(index, 30 | 60 | 90) {
            session_contents.push_str(&format!(
                "{{\"timestamp\":\"{timestamp}\",\"type\":\"compacted\",\"payload\":{{}}}}\n"
            ));
        }
    }
    tape.tee(&tape_contents).expect("merged tape is durable");
    fs::write(&session_path, &session_contents).expect("preserved session is durable");

    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e5"
        .parse()
        .expect("fixture lane");
    let author: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163ef"
        .parse()
        .expect("fixture author");
    let source_end = listen::scan(&tape_root, listen::Dialect::Codex, None)
        .expect("merged tape scans")
        .checkpoint;
    let session_source = session_compaction::history_checkpoint(&session_path)
        .expect("preserved session checkpoint reads");
    let history = DurableHistory {
        lanes: BTreeMap::from([(
            lane,
            DurableLane {
                tape: tape_root,
                worktree: root.join("worktree"),
                dialect: listen::Dialect::Codex,
                author,
                source: Some(source_end.clone()),
                session: Some(DurableSession {
                    path: session_path.clone(),
                    worktree: root.join("worktree"),
                    dialect: listen::Dialect::Codex,
                    thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
                    source: session_source,
                }),
            },
        )]),
        continuations: Mutex::new(HashMap::new()),
        hydration: Mutex::new(Hydration::default()),
        bus: None,
        steering: PresentationDirectives::default(),
        worktrees: SessionWorktrees::default(),
    };
    let future = wire::AgentMessage {
        lane,
        author,
        locus: wire::MessageLocus {
            member: source_end.next.member,
            offset: source_end.next.offset,
            length: 0,
            event: 0,
        },
        authored: "2031-04-12T18:44:09.271Z"
            .parse()
            .expect("future fixture instant"),
        authored_synthetic: false,
        observed: None,
        activity: wire::AgentActivityKind::Prose,
        final_answer: false,
        text: "future".to_owned(),
        classifications: Vec::new(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        boundary: None,
        media: Vec::new(),
    };
    MergedHistoryFixture {
        root,
        session_path,
        session_contents,
        lane,
        source_end,
        history,
        future,
    }
}

fn collect_merged_history_pages(
    fixture: &MergedHistoryFixture,
) -> (
    usize,
    Vec<usize>,
    Vec<wire::AgentMessage>,
    wire::AgentMessage,
) {
    let source = fixture
        .history
        .lanes
        .get(&fixture.lane)
        .expect("fixture lane source");
    let initial = fixture
        .history
        .tape_messages(
            fixture.lane,
            source,
            Some(&fixture.future),
            &fixture.source_end,
            wire::MESSAGE_FLOOR,
        )
        .expect("initial tape tail reads");
    let initial_before = initial.messages[0].clone();
    let mut before = initial_before.clone();
    let mut restored = Vec::new();
    let mut page_lengths = Vec::new();
    let mut pending_pages = 0;
    let mut prior_tape = before.locus.offset;
    let session_source = &source.session.as_ref().expect("fixture session").source;
    let mut prior_session = None;
    for _ in 0..128 {
        let page = canvas::stream::MessageHistoryProvider::history(
            &fixture.history,
            fixture.lane,
            Some(&before),
            canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
        )
        .expect("merged history reads")
        .expect("fixture lane is owned");
        if page.messages.is_empty() {
            assert!(page.more, "pending authentication keeps history open");
            let visible = session_source.next();
            let target = u64::try_from(fixture.session_contents.len())
                .expect("fixture session length fits u64");
            assert!(
                visible == 0 || visible == target,
                "authentication publishes only zero or the complete source target, found {visible}",
            );
            pending_pages += 1;
            continue;
        }
        assert!(!page.messages.is_empty());
        assert!(page.messages.len() <= canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT);
        page_lengths.push(page.messages.len());
        let oldest = page.messages[0].clone();
        restored.splice(0..0, page.messages);
        if !page.more {
            return (pending_pages, page_lengths, restored, initial_before);
        }
        before = oldest;
        let continuation = fixture
            .history
            .continuations
            .lock()
            .expect("continuation lock")
            .get(&(fixture.lane, Some(before.locus)))
            .cloned()
            .expect("oldest returned locus owns continuation");
        let tape_next = continuation.tape.expect("tape continuation").next.offset;
        let session_next = continuation.session.expect("session continuation").next();
        assert!(tape_next < prior_tape, "tape frontier strictly retreats");
        if let Some(prior_session) = prior_session {
            assert!(
                session_next <= prior_session,
                "session frontier retreats or holds after authentication publishes"
            );
        }
        prior_tape = tape_next;
        prior_session = Some(session_next);
    }
    panic!(
        "merged history did not exhaust after {} visible page(s) and {} restored rows; tape frontier {prior_tape}, session frontier {prior_session:?}; last page lengths {:?}",
        page_lengths.len(),
        restored.len(),
        &page_lengths[page_lengths.len().saturating_sub(8)..],
    )
}

fn assert_replaced_session_is_refused(
    fixture: &MergedHistoryFixture,
    initial_before: &wire::AgentMessage,
) {
    fs::remove_file(&fixture.session_path).expect("old preserved session is removed");
    fs::write(&fixture.session_path, &fixture.session_contents)
        .expect("replacement session is written");
    let error = canvas::stream::MessageHistoryProvider::history(
        &fixture.history,
        fixture.lane,
        Some(initial_before),
        canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
    )
    .expect_err("a replacement preserved session is refused");
    assert!(
        error.contains("preserved session history source changed"),
        "replacement diagnosis remains explicit: {error}",
    );
}

/// Write a Codex season of `messages` visible agent messages followed by
/// `tools` tool calls without a visible card, each record near 1.9 KiB.
fn season_tape(tag: &str, messages: usize, tools: usize) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("season tape opens");
    let filler = "x".repeat(1_800);
    let mut season = Vec::new();
    for index in 0..messages {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    for index in 0..tools {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.started\",\"item\":{{\"id\":\"call-{index}\",\"type\":\"command_execution\",\"command\":\"echo {filler}\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&season).expect("season is durable");
    root
}

fn season_history(root: &Path) -> (DurableHistory, CanonicalUuid, listen::Checkpoint) {
    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e5"
        .parse()
        .expect("fixture lane");
    let author: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163ef"
        .parse()
        .expect("fixture author");
    let source_end = listen::scan(root, listen::Dialect::Codex, None)
        .expect("season endpoint reads")
        .checkpoint;
    let history = DurableHistory {
        lanes: BTreeMap::from([(
            lane,
            DurableLane {
                tape: root.to_path_buf(),
                worktree: root.join("worktree"),
                dialect: listen::Dialect::Codex,
                author,
                source: Some(source_end.clone()),
                session: None,
            },
        )]),
        continuations: Mutex::new(HashMap::new()),
        hydration: Mutex::new(Hydration::default()),
        bus: None,
        steering: PresentationDirectives::default(),
        worktrees: SessionWorktrees::default(),
    };
    (history, lane, source_end)
}

#[test]
fn startup_tail_reads_one_fixed_window_before_byte_zero() {
    // Eight hundred records exceed the 1 MiB startup window, while many
    // initial pages fit inside it. Cold replay reads that one window and
    // leaves the rest to pageable history.
    const DENSE_LANE_MESSAGES: usize = 800;
    let root = season_tape("tail", DENSE_LANE_MESSAGES, 0);

    let tail = tail_backlog(&root, listen::Dialect::Codex).expect("season tail scans");
    assert!(
        visible_cards(&tail.scan) > wire::MESSAGE_FLOOR,
        "one startup window carries more than the initial page of a dense lane",
    );
    assert!(visible_cards(&tail.scan) < DENSE_LANE_MESSAGES);
    assert!(tail.work.source_bytes <= startup_source_bytes(listen::Dialect::Codex));
    assert!(tail.work.source_bytes > STARTUP_WINDOW_BYTES);
    assert_eq!(tail.work.records, tail.scan.records.len());
    assert!(tail.work.passes <= codex_startup_passes());
    let first_offset = tail
        .scan
        .records
        .first()
        .and_then(|record| match &record.payload {
            listen::RecordPayload::Events(events) => events.first(),
            listen::RecordPayload::Residue(_) => None,
        })
        .and_then(|event| event.spans.first())
        .map(|span| span.offset)
        .expect("tail has a first sourced event");
    assert!(first_offset > 0, "startup does not scan back to byte zero");

    fs::remove_dir_all(root).expect("season fixture cleanup");
}

#[test]
fn startup_tail_affords_one_captured_record_at_its_limit() {
    // Enough newest messages that the startup window begins deep inside a
    // record at the capture limit: the boundary allowance retreats over the
    // rest of that record, and the window is read from where it was asked.
    const NEWEST_MESSAGES: usize = 500;
    let root = season_tape("limit-record", 30, 0);
    let tape = tape::Tape::open(&root).expect("season tape reopens");
    let mut record = vec![b'x'; lane_surface::CAPTURE_LIMIT];
    record.push(b'\n');
    tape.tee(&record)
        .expect("a record at the capture limit is durable");
    let filler = "x".repeat(1_800);
    let mut newest = Vec::new();
    for index in 0..NEWEST_MESSAGES {
        newest.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"late-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&newest).expect("newest messages are durable");

    let tail = tail_backlog(&root, listen::Dialect::Codex)
        .expect("a record at the capture limit is affordable");
    let straddled = tail.scan.records.first().expect("the window has records");
    assert_eq!(
        straddled.locus.length,
        record.len() as u64,
        "the window begins with the whole straddled record"
    );
    assert_eq!(tail.scan.records.len(), NEWEST_MESSAGES + 1);
    assert!(tail.work.source_bytes > STARTUP_WINDOW_BYTES + STARTUP_WINDOW_BYTES / 2);
    assert!(tail.work.source_bytes <= startup_source_bytes(listen::Dialect::Codex));
    assert!(tail.work.passes <= codex_startup_passes());

    fs::remove_dir_all(root).expect("limit-record fixture cleanup");
}

/// Bounded reads one Codex startup scan may perform under its own allowance.
fn codex_startup_passes() -> usize {
    tail_passes(startup_source_bytes(listen::Dialect::Codex))
}

#[test]
fn startup_tail_crosses_one_reassembled_record_at_the_admitted_logical_limit() {
    // OMP capture tees a reassembled logical record, so the newest window can
    // begin inside a record far longer than one physical RPC frame. The
    // boundary allowance retreats over the whole of it and the window still
    // decodes every newer message.
    const NEWEST_MESSAGES: usize = 200;
    let root = omp_tape("omp-limit-record");
    let tape = tape::Tape::open(&root).expect("omp tape reopens");
    tape.tee(omp_messages(30).as_bytes())
        .expect("older messages are durable");
    let head =
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":""#;
    let foot = r#""}],"timestamp":1789525452596}}"#;
    // Sized from what capture may tee, not from the allowance under test: a
    // budget that shrinks back to the physical frame cannot cross this.
    let admitted = vendor::omp::frame::MAX_REASSEMBLED_BYTES + 1;
    let filler = admitted - 1 - head.len() - foot.len();
    let mut record = String::with_capacity(admitted);
    record.push_str(head);
    record.extend(std::iter::repeat_n('x', filler));
    record.push_str(foot);
    record.push('\n');
    assert_eq!(record.len(), admitted);
    tape.tee(record.as_bytes())
        .expect("a record at the logical limit is durable");
    tape.tee(omp_messages(NEWEST_MESSAGES).as_bytes())
        .expect("newest messages are durable");

    let tail = tail_backlog(&root, listen::Dialect::Omp)
        .expect("a reassembled record at its limit is affordable");
    let straddled = tail.scan.records.first().expect("the window has records");
    assert_eq!(
        straddled.locus.length,
        record.len() as u64,
        "the window begins with the whole straddled record",
    );
    assert_eq!(tail.scan.records.len(), NEWEST_MESSAGES + 1);
    assert!(tail.work.source_bytes <= startup_source_bytes(listen::Dialect::Omp));
    assert!(tail.work.passes <= tail_passes(startup_source_bytes(listen::Dialect::Omp)));

    fs::remove_dir_all(root).expect("omp limit-record fixture cleanup");
}

#[test]
fn a_record_beyond_the_dialects_admitted_length_refuses_within_its_allowance() {
    // Nothing admitted can land a Codex record this long, so the retreat runs
    // out of source bytes rather than reading the tape without a bound. The
    // refusal names the dimension it spent and reports the work it did.
    let root = season_tape("oversize-record", 30, 0);
    let tape = tape::Tape::open(&root).expect("season tape reopens");
    // The retreat pays for every byte of the record below the window it was
    // asked for, so the record has to outrun the whole allowance, not the
    // admitted length alone.
    let mut oversize = vec![
        b'x';
        startup_source_bytes(listen::Dialect::Codex) as usize
            + 2 * STARTUP_WINDOW_BYTES as usize
    ];
    oversize.push(b'\n');
    tape.tee(&oversize).expect("the long record is durable");

    let error = tail_backlog(&root, listen::Dialect::Codex)
        .expect_err("a record beyond the admitted length is refused");
    let listen::ScanError::WorkLimit {
        dimension,
        limit,
        work,
        ..
    } = &error
    else {
        panic!("the refusal names the allowance it spent: {error}");
    };
    assert_eq!(*dimension, "source bytes");
    assert_eq!(*limit, startup_source_bytes(listen::Dialect::Codex));
    assert!(work.source_bytes <= startup_source_bytes(listen::Dialect::Codex));
    assert!(work.passes <= codex_startup_passes());

    fs::remove_dir_all(root).expect("oversize fixture cleanup");
}

/// Write an empty tape for OMP records and return its root.
fn omp_tape(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    tape::Tape::open(&root).expect("omp tape opens");
    root
}

/// `count` visible OMP assistant messages, each near 1.9 KiB.
fn omp_messages(count: usize) -> String {
    let filler = "x".repeat(1_800);
    let mut messages = String::new();
    for index in 0..count {
        messages.push_str(&format!(
            "{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{index} {filler}\"}}],\"timestamp\":178952545{index}}}}}\n"
        ));
    }
    messages
}

#[test]
fn tool_only_tail_stops_at_one_fixed_read_with_a_backward_cursor() {
    let root = season_tape("sparse-tail", 30, SPARSE_TOOL_RECORDS);

    let tail = tail_backlog(&root, listen::Dialect::Codex).expect("sparse tail scans");
    assert_eq!(
        visible_cards(&tail.scan),
        0,
        "one startup window need not cross sparse tools to reach a visible card",
    );
    assert!(
        tail.scan.records.len() < SPARSE_TOOL_RECORDS,
        "the scanner does not cross the complete sparse tool tail",
    );
    assert!(tail.start.next > listen::Position::start());
    assert!(tail.work.source_bytes <= startup_source_bytes(listen::Dialect::Codex));
    assert_eq!(tail.work.records, tail.scan.records.len());
    assert!(tail.work.passes <= codex_startup_passes());

    fs::remove_dir_all(root).expect("sparse fixture cleanup");
}

#[test]
fn historical_page_wades_through_a_sparse_tool_horizon_to_fill_the_page() {
    // The tool horizon outgrows one history window, so the first window
    // carries no visible card and the request must chain an older one.
    let root = season_tape("sparse-history", 300, SPARSE_HORIZON_RECORDS);
    let (history, lane, source_end) = season_history(&root);
    let source = history.lanes.get(&lane).expect("season lane");
    let limit = canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT;

    let newest = history
        .tape_messages(lane, source, None, &source_end, limit)
        .expect("newest history window reads");
    assert!(newest.messages.is_empty(), "one window holds only tools");
    assert!(newest.more);
    assert!(newest.start.next > listen::Position::start());
    assert!(newest.work.records < SPARSE_HORIZON_RECORDS);
    let newest_start = newest.start.next;
    let newest_end = newest.end.clone();

    let filled = history
        .fill_tape_messages(lane, source, None, limit, newest)
        .expect("history fills across windows");
    assert_eq!(
        filled.messages.len(),
        limit,
        "the page reaches its visible target"
    );
    assert!(filled.more, "older messages remain behind the page");
    assert!(
        filled.start.next < newest_start,
        "the request read older windows"
    );
    assert_eq!(filled.end, newest_end);
    let window = history_source_bytes(listen::Dialect::Codex);
    assert!(filled.work.source_bytes > window);
    assert!(filled.work.source_bytes <= window * HISTORY_READ_LIMIT as u64);
    assert!(filled.work.records > SPARSE_HORIZON_RECORDS);
    assert!(filled.messages[0].text.starts_with("250 "));
    assert!(filled.messages[limit - 1].text.starts_with("299 "));
    assert!(
        filled
            .messages
            .windows(2)
            .all(|pair| pair[0].chronology_key() < pair[1].chronology_key()),
    );

    let page = canvas::stream::MessageHistoryProvider::history(&history, lane, None, limit)
        .expect("served history reads")
        .expect("season lane is owned");
    assert_eq!(page.messages.len(), limit);
    assert!(page.more);
    let oldest = page.messages[0].clone();
    let continuation = history
        .continuations
        .lock()
        .expect("continuation lock")
        .get(&(lane, Some(oldest.locus)))
        .cloned()
        .expect("the served page keeps its continuation");
    assert_eq!(
        continuation.tape.expect("tape continuation").next.offset,
        oldest.locus.offset,
        "the next request resumes at the oldest served card, not behind the horizon",
    );
    let older =
        canvas::stream::MessageHistoryProvider::history(&history, lane, Some(&oldest), limit)
            .expect("older history reads")
            .expect("season lane is owned");
    assert_eq!(older.messages.len(), limit);
    assert!(older.messages[0].text.starts_with("200 "));

    fs::remove_dir_all(root).expect("sparse history fixture cleanup");
}

#[test]
fn history_request_stops_at_the_tape_start_without_a_visible_card() {
    let root = season_tape("tool-only-history", 0, SPARSE_TOOL_RECORDS);
    let (history, lane, source_end) = season_history(&root);
    let source = history.lanes.get(&lane).expect("season lane");
    let limit = canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT;

    let newest = history
        .tape_messages(lane, source, None, &source_end, limit)
        .expect("newest history window reads");
    let filled = history
        .fill_tape_messages(lane, source, None, limit, newest)
        .expect("history reaches the tape start");
    assert!(filled.messages.is_empty());
    assert!(
        !filled.more,
        "a tape with no visible card is exhausted, not pending"
    );
    assert_eq!(filled.start.next, listen::Position::start());
    assert_eq!(filled.work.records, SPARSE_TOOL_RECORDS);

    fs::remove_dir_all(root).expect("tool-only history fixture cleanup");
}

#[test]
fn merged_history_pages_retreat_both_sources_without_gaps_or_rescans() {
    let fixture = merged_history_fixture();
    let (pending_pages, page_lengths, restored, initial_before) =
        collect_merged_history_pages(&fixture);

    assert!(
        pending_pages > 0,
        "session history waits for authentication"
    );
    assert!(
        page_lengths
            .iter()
            .all(|length| *length <= canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT),
    );
    assert_eq!(
        restored.len(),
        140 - wire::MESSAGE_FLOOR + 3,
        "every message before the initial page and every compaction is restored",
    );
    assert_eq!(
        restored
            .iter()
            .filter(|message| message.activity == wire::AgentActivityKind::Compaction)
            .count(),
        3,
    );
    assert!(
        restored
            .windows(2)
            .all(|pair| pair[0].chronology_key() < pair[1].chronology_key()),
        "merged pages remain in strict shared chronology",
    );
    assert_eq!(
        restored
            .iter()
            .map(|message| (message.locus, message.activity))
            .collect::<std::collections::HashSet<_>>()
            .len(),
        restored.len(),
        "source continuation returns no duplicate card identities",
    );
    let prose = restored
        .iter()
        .filter(|message| message.activity == wire::AgentActivityKind::Prose)
        .collect::<Vec<_>>();
    assert_eq!(prose.len(), 140 - wire::MESSAGE_FLOOR);
    for (index, message) in prose.into_iter().enumerate() {
        assert!(
            message.text.starts_with(&format!("message {index} ")),
            "every tape message before the initial visible boundary is restored exactly",
        );
    }

    assert_replaced_session_is_refused(&fixture, &initial_before);
    fs::remove_dir_all(&fixture.root).expect("merged fixture cleanup");
}

/// Season a preserved Codex session whose newest `groups` compactions each sit
/// behind five near 1.9 KiB assistant records, so a page of compaction cards
/// spans more source bytes than one history window.
fn season_session_horizon(root: &Path, groups: usize) -> (DurableHistory, CanonicalUuid) {
    let session_path = root.join("rollout.jsonl");
    fs::create_dir_all(root).expect("session horizon fixture root exists");
    let filler = "x".repeat(1_800);
    let mut group_contents = Vec::new();
    for group in 0..groups {
        let mut contents = String::new();
        for slot in 0..5 {
            let timestamp = timestamp(group * 6 + slot);
            contents.push_str(&format!(
                "{{\"timestamp\":\"{timestamp}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"message {group} {slot} {filler}\"}}]}}}}\n"
            ));
        }
        let timestamp = timestamp(group * 6 + 5);
        contents.push_str(&format!(
            "{{\"timestamp\":\"{timestamp}\",\"type\":\"compacted\",\"payload\":{{}}}}\n"
        ));
        group_contents.push(contents);
    }
    let newest_page: usize = group_contents
        .iter()
        .rev()
        .take(canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT)
        .map(String::len)
        .sum();
    assert!(
        newest_page > session_compaction::HISTORY_BYTE_BUDGET,
        "one page of compactions outgrows one history window: {newest_page} bytes",
    );
    fs::write(&session_path, group_contents.concat()).expect("preserved session is durable");

    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e5"
        .parse()
        .expect("fixture lane");
    let author: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163ef"
        .parse()
        .expect("fixture author");
    let session_source = session_compaction::history_checkpoint(&session_path)
        .expect("preserved session checkpoint reads");
    let history = DurableHistory {
        lanes: BTreeMap::from([(
            lane,
            DurableLane {
                tape: root.join("tape"),
                worktree: root.join("worktree"),
                dialect: listen::Dialect::Codex,
                author,
                source: None,
                session: Some(DurableSession {
                    path: session_path,
                    worktree: root.join("worktree"),
                    dialect: listen::Dialect::Codex,
                    thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
                    source: session_source,
                }),
            },
        )]),
        continuations: Mutex::new(HashMap::new()),
        hydration: Mutex::new(Hydration::default()),
        bus: None,
        steering: PresentationDirectives::default(),
        worktrees: SessionWorktrees::default(),
    };
    (history, lane)
}

#[test]
fn session_history_chains_windows_until_one_request_fills_its_page() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-session-horizon-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let (history, lane) = season_session_horizon(&root, 60);

    let page = (0..128)
        .find_map(|_| {
            let page = canvas::stream::MessageHistoryProvider::history(
                &history,
                lane,
                None,
                canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
            )
            .expect("session history reads")
            .expect("fixture lane is owned");
            if page.messages.is_empty() {
                assert!(page.more, "pending authentication keeps history open");
                return None;
            }
            Some(page)
        })
        .expect("session authentication settles before the page is served");

    assert_eq!(
        page.messages.len(),
        canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
        "one request chains older windows until the page limit is met",
    );
    assert!(page.more, "older compactions remain behind the page");
    assert!(
        page.messages
            .iter()
            .all(|message| message.activity == wire::AgentActivityKind::Compaction),
        "the preserved session serves only compaction cards",
    );
    assert!(
        page.messages
            .windows(2)
            .all(|pair| pair[0].chronology_key() < pair[1].chronology_key()),
        "chained windows keep strict source chronology",
    );
    let newest = timestamp(59 * 6 + 5)
        .parse()
        .expect("newest fixture instant");
    assert!(
        page.messages
            .last()
            .is_some_and(|message| message.authored == newest),
        "the page ends at the newest compaction",
    );
    fs::remove_dir_all(&root).expect("session horizon fixture cleanup");
}

/// One compaction both sources recorded is one boundary.
///
/// omp announces a compaction on the stream Spice captures and writes it again
/// into its own session file. Both records state the same vendor identity, so
/// the merged page keeps the preserved entry, which is the record that carries
/// the vendor's own timestamp. A compaction only the stream recorded keeps its
/// announcement even when another compaction repeats its summary.
#[test]
fn a_compaction_both_sources_recorded_is_one_preserved_boundary() {
    let (root, history, lane, future) = paired_boundary_fixture();
    let page = (0..128)
        .find_map(|_| {
            let page = canvas::stream::MessageHistoryProvider::history(
                &history,
                lane,
                Some(&future),
                canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
            )
            .expect("merged history reads")
            .expect("fixture lane is owned");
            (!page.messages.is_empty()).then_some(page)
        })
        .expect("session authentication settles before the page is served");
    let boundaries = page
        .messages
        .iter()
        .filter(|message| message.activity == wire::AgentActivityKind::Compaction)
        .collect::<Vec<_>>();
    assert_eq!(
        boundaries.len(),
        2,
        "the paired records collapse and the unpaired announcement remains: {boundaries:#?}",
    );
    let paired = boundary_card(&boundaries, "kept:e1:900000");
    assert!(
        paired.boundary.as_ref().is_some_and(|it| it.preserved),
        "the vendor's own record represents the compaction it also announced",
    );
    assert!(
        !paired.authored_synthetic,
        "the preserved record carries the vendor timestamp the announcement lacks",
    );
    let unpaired = boundary_card(&boundaries, "kept:e9:910000");
    assert!(
        unpaired.boundary.as_ref().is_some_and(|it| !it.preserved),
        "a compaction only the stream recorded keeps its announcement",
    );
    assert_eq!(
        unpaired.text, paired.text,
        "the two boundaries are distinguished by identity, not by summary text",
    );
    fs::remove_dir_all(root).expect("paired boundary fixture cleanup");
}

fn boundary_card<'a>(boundaries: &[&'a wire::AgentMessage], id: &str) -> &'a wire::AgentMessage {
    boundaries
        .iter()
        .copied()
        .find(|message| message.boundary.as_ref().is_some_and(|it| it.id == id))
        .unwrap_or_else(|| panic!("the page serves boundary {id}: {boundaries:#?}"))
}

/// A lane whose captured tape and preserved session both recorded one
/// compaction, beside a second compaction only the tape recorded that repeats
/// the same summary.
fn paired_boundary_fixture() -> (PathBuf, DurableHistory, CanonicalUuid, wire::AgentMessage) {
    const TAPE: &str = concat!(
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"before"}],"timestamp":1789525452596}}"#,
        "\n",
        r#"{"type":"auto_compaction_end","action":"context-full","result":{"summary":"so far","firstKeptEntryId":"e1","tokensBefore":900000},"aborted":false}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"between"}],"timestamp":1789525453596}}"#,
        "\n",
        r#"{"type":"auto_compaction_end","action":"context-full","result":{"summary":"so far","firstKeptEntryId":"e9","tokensBefore":910000},"aborted":false}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"after"}],"timestamp":1789525454596}}"#,
        "\n",
    );
    const SESSION: &str = concat!(
        r#"{"type":"session","version":3,"id":"01a0a807-666f-7524-be0d-2ee476b6c15c","timestamp":"2026-09-16T02:24:12.399Z","cwd":"/work/tree"}"#,
        "\n",
        r#"{"type":"compaction","id":"k1","parentId":"d1","timestamp":"2026-09-16T06:43:23.000Z","summary":"so far","firstKeptEntryId":"e1","tokensBefore":900000}"#,
        "\n",
    );
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-paired-boundary-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape_root = root.join("tape");
    let session_path = root.join("session.jsonl");
    fs::create_dir_all(&root).expect("paired fixture root exists");
    let tape = tape::Tape::open(&tape_root).expect("paired tape opens");
    tape.tee(TAPE.as_bytes()).expect("paired tape is durable");
    fs::write(&session_path, SESSION).expect("preserved session is durable");
    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e5"
        .parse()
        .expect("fixture lane");
    let author: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163ef"
        .parse()
        .expect("fixture author");
    let source_end = listen::scan(&tape_root, listen::Dialect::Omp, None)
        .expect("paired tape scans")
        .checkpoint;
    let session_source = session_compaction::history_checkpoint(&session_path)
        .expect("preserved session checkpoint reads");
    let history = DurableHistory {
        lanes: BTreeMap::from([(
            lane,
            DurableLane {
                tape: tape_root,
                worktree: root.join("worktree"),
                dialect: listen::Dialect::Omp,
                author,
                source: Some(source_end.clone()),
                session: Some(DurableSession {
                    path: session_path,
                    worktree: root.join("worktree"),
                    dialect: listen::Dialect::Omp,
                    thread: "01a0a807-666f-7524-be0d-2ee476b6c15c".to_owned(),
                    source: session_source,
                }),
            },
        )]),
        continuations: Mutex::new(HashMap::new()),
        hydration: Mutex::new(Hydration::default()),
        bus: None,
        steering: PresentationDirectives::default(),
        worktrees: SessionWorktrees::default(),
    };
    let future = wire::AgentMessage {
        lane,
        author,
        locus: wire::MessageLocus {
            member: source_end.next.member,
            offset: source_end.next.offset,
            length: 0,
            event: 0,
        },
        authored: "2031-04-12T18:44:09.271Z"
            .parse()
            .expect("future fixture instant"),
        authored_synthetic: false,
        observed: None,
        activity: wire::AgentActivityKind::Prose,
        final_answer: false,
        text: "future".to_owned(),
        classifications: Vec::new(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        boundary: None,
        media: Vec::new(),
    };
    (root, history, lane, future)
}
