use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

const SPARSE_TOOL_RECORDS: usize = 400;

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
        observations: MessageObservations::default(),
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
        badges: Default::default(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
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
) -> (Vec<usize>, Vec<wire::AgentMessage>, wire::AgentMessage) {
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
            &fixture.future,
            &fixture.source_end,
            canvas::stream::INITIAL_MESSAGE_LIMIT,
        )
        .expect("initial tape tail reads");
    let initial_before = initial.messages[0].clone();
    let mut before = initial_before.clone();
    let mut restored = Vec::new();
    let mut page_lengths = Vec::new();
    let mut prior_tape = before.locus.offset;
    let mut prior_session = source
        .session
        .as_ref()
        .expect("fixture session")
        .source
        .next();
    loop {
        let page = canvas::stream::MessageHistoryProvider::history(
            &fixture.history,
            fixture.lane,
            &before,
            canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
        )
        .expect("merged history reads")
        .expect("fixture lane is owned");
        assert!(!page.messages.is_empty());
        assert!(page.messages.len() <= canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT);
        page_lengths.push(page.messages.len());
        before = page.messages[0].clone();
        let continuation = fixture
            .history
            .continuations
            .lock()
            .expect("continuation lock")
            .get(&(fixture.lane, before.locus))
            .cloned()
            .expect("oldest returned locus owns continuation");
        let tape_next = continuation.tape.expect("tape continuation").next.offset;
        let session_next = continuation.session.expect("session continuation").next();
        assert!(tape_next < prior_tape, "tape frontier strictly retreats");
        assert!(
            session_next < prior_session,
            "session frontier strictly retreats"
        );
        prior_tape = tape_next;
        prior_session = session_next;
        restored.splice(0..0, page.messages);
        if !page.more {
            break;
        }
    }
    (page_lengths, restored, initial_before)
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
        initial_before,
        canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
    )
    .expect_err("a replacement preserved session is refused");
    assert!(
        error.contains("preserved session history source changed"),
        "replacement diagnosis remains explicit: {error}",
    );
}

#[test]
fn visible_card_target_stops_a_season_tail_before_byte_zero() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-tail-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("season tape opens");
    // Three hundred records exceed the first 32 KiB window, while more than
    // one initial page fits inside it. Cold replay must stop there rather than
    // expanding until the complete retained history is materialized.
    let filler = "x".repeat(1_800);
    let mut season = Vec::new();
    for index in 0..300 {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&season).expect("season is durable");

    let scan = tail_backlog(&root, listen::Dialect::Codex).expect("season tail scans");
    assert!(
        visible_cards(&scan) > canvas::stream::INITIAL_MESSAGE_LIMIT,
        "the selected tail supplies one initial page and its look-behind card",
    );
    assert!(
        visible_cards(&scan) < canvas::stream::MESSAGE_CAPACITY,
        "cold replay does not materialize the retained-history horizon",
    );
    let first_offset = scan
        .records
        .first()
        .and_then(|record| match &record.payload {
            listen::RecordPayload::Events(events) => events.first(),
            listen::RecordPayload::Residue(_) => None,
        })
        .and_then(|event| event.spans.first())
        .map(|span| span.offset)
        .expect("tail has a first sourced event");
    assert!(
        first_offset > 0,
        "the visible-card target is reached without scanning byte zero",
    );

    fs::remove_dir_all(root).expect("season fixture cleanup");
}

#[test]
fn tool_only_tail_expands_until_the_initial_visible_budget_is_satisfied() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-sparse-tail-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("sparse tape opens");
    let filler = "x".repeat(1_800);
    let mut season = Vec::new();
    for index in 0..30 {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    for index in 0..SPARSE_TOOL_RECORDS {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.started\",\"item\":{{\"id\":\"call-{index}\",\"type\":\"command_execution\",\"command\":\"echo {filler}\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&season).expect("sparse season is durable");

    let scan = tail_backlog(&root, listen::Dialect::Codex).expect("sparse tail scans");
    assert_eq!(
        visible_cards(&scan),
        30,
        "tool activity does not consume the visible-card budget",
    );
    assert_eq!(
        scan.records.len(),
        430,
        "the scanner crosses the sparse tool tail to reach visible cards",
    );

    fs::remove_dir_all(root).expect("sparse fixture cleanup");
}

#[test]
fn historical_page_crosses_sparse_tools_without_materializing_the_horizon() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-sparse-history-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("sparse history tape opens");
    let filler = "x".repeat(1_800);
    let mut season = Vec::new();
    for index in 0..300 {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    for index in 0..SPARSE_TOOL_RECORDS {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.started\",\"item\":{{\"id\":\"call-{index}\",\"type\":\"command_execution\",\"command\":\"echo {filler}\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&season).expect("sparse history is durable");
    let end = listen::scan(&root, listen::Dialect::Codex, None)
        .expect("sparse history endpoint reads")
        .checkpoint;

    let tail = history_tail(
        &root,
        listen::Dialect::Codex,
        &end,
        canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
    )
    .expect("sparse history page scans");
    assert!(
        tail.start.next > listen::Position::start(),
        "the historical page stops before byte zero",
    );
    assert!(
        tail.scan.records.len() > SPARSE_TOOL_RECORDS,
        "non-visible tools are traversed to reach the visible target",
    );
    assert!(
        visible_cards(&tail.scan) < canvas::stream::MESSAGE_CAPACITY,
        "the source scan does not reconstruct the full retained horizon",
    );

    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e5"
        .parse()
        .expect("fixture lane");
    let author: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163ef"
        .parse()
        .expect("fixture author");
    let messages = CollectedMessages::new(canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT);
    let mut reader = BacklogReader::default();
    let mut diagnostic = Vec::new();
    publish_scan(
        tail.scan,
        lane,
        author,
        &messages,
        &mut reader,
        &mut diagnostic,
    )
    .expect("sparse history materializes");
    assert!(diagnostic.is_empty());
    assert_eq!(
        messages
            .inner
            .into_inner()
            .expect("history lock")
            .messages
            .len(),
        canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT,
        "only the requested visible page is materialized",
    );

    fs::remove_dir_all(root).expect("sparse history fixture cleanup");
}

#[test]
fn merged_history_pages_retreat_both_sources_without_gaps_or_rescans() {
    let fixture = merged_history_fixture();
    let (page_lengths, restored, initial_before) = collect_merged_history_pages(&fixture);

    assert_eq!(page_lengths, [50, 50, 18]);
    assert_eq!(restored.len(), 118);
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
    assert!(restored[0].text.starts_with("message 0 "));
    assert!(restored[117].text.starts_with("message 114 "));

    assert_replaced_session_is_refused(&fixture, &initial_before);
    fs::remove_dir_all(&fixture.root).expect("merged fixture cleanup");
}
