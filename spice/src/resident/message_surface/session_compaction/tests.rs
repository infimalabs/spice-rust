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
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        if let Some(existing) = messages
            .iter()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        {
            return if existing == &message {
                Ok(false)
            } else {
                Err(canvas::stream::MessagePublishError::IdentityConflict)
            };
        }
        messages.push(message);
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        let Some(existing) = messages
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(canvas::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
}

#[test]
fn real_codex_rollout_compaction_is_published_once_from_complete_jsonl() {
    let scratch = Scratch::new("codex");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"timestamp\":\"2030-04-12T18:44:08.000Z\",\"type\":\"response_item\",",
            "\"payload\":{\"type\":\"message\",\"role\":\"assistant\",",
            "\"content\":[{\"type\":\"output_text\",\"text\":\"before\"}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\",",
            "\"payload\":{\"message\":\"\",\"replacement_history\":[",
            "{\"type\":\"message\",\"role\":\"user\",\"content\":[]},",
            "{\"type\":\"compaction\",\"encrypted_content\":\"ciphertext\"}],",
            "\"window_number\":7}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.272Z\",\"type\":\"event_msg\",",
            "\"payload\":{\"type\":\"context_compacted\"}}\n",
        ),
    )
    .expect("real-format Codex transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    let records = scan(&mut cursor).expect("session transcript scans");
    assert_eq!(records.len(), 1, "only the completed boundary is visible");
    assert_eq!(records[0].authored, "2030-04-12T18:44:09.271Z");
    assert!(matches!(records[0].decoded, listen::Event::Compaction(_)));

    let publisher = Messages::default();
    publish(
        records.into_iter().next().expect("one compaction"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        Path::new("/worktree"),
        &publisher,
        &mut MessageState::default(),
    )
    .expect("typed boundary publishes");
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert_eq!(messages[0].text, "Context compacted");
    assert!(messages[0].unavailable_tool_terminals.is_empty());
    drop(messages);

    assert!(
        scan(&mut cursor)
            .expect("unchanged transcript scans")
            .is_empty(),
        "the cursor does not replay a consumed boundary",
    );
}

#[test]
fn codex_recovery_precedes_display_and_display_only_replay_is_inert() {
    const THREAD: &str = "018f7777-7777-7444-8888-999999999999";
    let scratch = Scratch::new("codex-recovery");
    let transcript = scratch
        .0
        .join("sessions")
        .join(format!("rollout-{THREAD}.jsonl"));
    fs::create_dir_all(transcript.parent().expect("transcript parent"))
        .expect("Codex session directory exists");
    fs::write(
        &transcript,
        b"{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n",
    )
    .expect("Codex boundary is written");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let author = uuid("018f82d4-6f59-7d06-92e3-bce9131163ef");
    let residency = session_residency_for(lane, author, THREAD, "codex");
    let publisher = Messages::default();
    let mut state = observed_state(lane);
    let mut diagnostic = Vec::new();
    let mut recovered = Vec::new();
    let mut compactions = SessionCompactions::new(codex_environment(&scratch.0));

    compactions
        .synchronize_with(
            &residency,
            &publisher,
            &mut state,
            &mut diagnostic,
            |recovered_lane, occupant, thread, locus| {
                assert!(
                    publisher.0.lock().expect("message lock").is_empty(),
                    "durable recovery precedes the fallible Canvas projection",
                );
                recovered.push((recovered_lane, occupant, thread.to_owned(), locus));
                Ok(())
            },
        )
        .expect("Codex synchronization succeeds");

    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].0, lane);
    assert_eq!(recovered[0].1, author);
    assert_eq!(recovered[0].2, THREAD);
    assert_eq!(recovered[0].3.member, session_member(THREAD));
    assert_eq!(publisher.0.lock().expect("message lock").len(), 1);

    let mut display_only = SessionCompactions::new(codex_environment(&scratch.0));
    display_only
        .synchronize_display_only(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("display-only replay remains non-authoritative");
    assert_eq!(
        recovered.len(),
        1,
        "Canvas replay does not recover authority"
    );
    assert_eq!(
        publisher.0.lock().expect("message lock").len(),
        1,
        "Canvas identity still deduplicates the replay",
    );
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
}

#[test]
fn codex_startup_recovers_latest_boundary_beyond_the_canvas_tail_once() {
    const THREAD: &str = "018f7777-7777-7444-8888-999999999999";
    const OLD_BOUNDARY: &str =
        "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n";
    const LATEST_BOUNDARY: &str =
        "{\"timestamp\":\"2030-04-12T19:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n";
    const LIVE_BOUNDARY: &str =
        "{\"timestamp\":\"2030-04-12T20:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n";
    let scratch = Scratch::new("codex-recovery-before-tail");
    let transcript = scratch
        .0
        .join("sessions")
        .join(format!("rollout-{THREAD}.jsonl"));
    fs::create_dir_all(transcript.parent().expect("transcript parent"))
        .expect("Codex session directory exists");
    let filler = "x".repeat(1_800);
    let mut contents = OLD_BOUNDARY.to_owned();
    for index in 0..40 {
        contents.push_str(&codex_assistant_record(index, &filler));
    }
    let latest_offset = u64::try_from(contents.len()).expect("fixture offset fits u64");
    contents.push_str(LATEST_BOUNDARY);
    for index in 40..340 {
        contents.push_str(&codex_assistant_record(index, &filler));
    }
    fs::write(&transcript, &contents).expect("long Codex transcript is written");

    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let author = uuid("018f82d4-6f59-7d06-92e3-bce9131163ef");
    let residency = session_residency_for(lane, author, THREAD, "codex");
    let publisher = Messages::default();
    let mut state = observed_state(lane);
    let mut diagnostic = Vec::new();
    let mut recovered = Vec::new();
    let mut compactions = SessionCompactions::new(codex_environment(&scratch.0));

    for _ in 0..2 {
        compactions
            .synchronize_with(
                &residency,
                &publisher,
                &mut state,
                &mut diagnostic,
                |_, _, _, locus| {
                    recovered.push(locus);
                    Ok(())
                },
            )
            .expect("Codex startup synchronization succeeds");
    }

    assert_eq!(recovered.len(), 1, "startup recovery is not replayed");
    assert_eq!(recovered[0].member, session_member(THREAD));
    assert_eq!(recovered[0].offset, latest_offset);
    assert_eq!(
        recovered[0].length,
        u64::try_from(LATEST_BOUNDARY.len()).expect("boundary length fits u64")
    );
    assert!(
        publisher.0.lock().expect("message lock").is_empty(),
        "the boundary outside the Canvas tail is not displayed",
    );

    append(&transcript, LIVE_BOUNDARY.as_bytes());
    compactions
        .synchronize_with(
            &residency,
            &publisher,
            &mut state,
            &mut diagnostic,
            |_, _, _, locus| {
                recovered.push(locus);
                Ok(())
            },
        )
        .expect("live Codex synchronization succeeds");
    assert_eq!(recovered.len(), 2, "live append scanning remains active");
    assert_eq!(
        publisher.0.lock().expect("message lock").len(),
        1,
        "the live boundary remains visible",
    );
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
}

#[test]
fn codex_startup_read_volume_depends_on_the_tail_not_older_epochs() {
    const BOUNDARY: &str =
        "{\"timestamp\":\"2030-04-12T19:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n";
    let scratch = Scratch::new("codex-bounded-startup");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut older = String::new();
    let mut expected_decoded = None;
    for epochs in [1, 40] {
        for _ in 0..epochs * 100 {
            older.push_str(&codex_assistant_record(0, &filler));
        }
        let latest_offset = older.len() as u64;
        let mut contents = older.clone();
        contents.push_str(BOUNDARY);
        for index in 0..40 {
            contents.push_str(&codex_assistant_record(index, &filler));
        }
        fs::write(&transcript, &contents).expect("growing session is written");
        let mut cursor = cursor(&transcript, listen::Dialect::Codex);
        cursor.initialized = false;
        BOOTSTRAP_BYTES_DECODED.with(|count| count.set(0));

        let records = scan(&mut cursor).expect("bounded startup scans");

        assert_eq!(
            cursor.startup_recovery.expect("latest boundary").offset,
            latest_offset
        );
        assert!(
            records.is_empty(),
            "the older boundary is outside the initial 25 cards"
        );
        let decoded = BOOTSTRAP_BYTES_DECODED.with(Cell::get);
        assert_eq!(*expected_decoded.get_or_insert(decoded), decoded);
        assert!(
            decoded < 8 * INITIAL_WINDOW_BYTES,
            "startup decoded {decoded} bytes"
        );
        assert_eq!(cursor.next, contents.len() as u64);
    }
}

#[test]
fn reverse_compaction_pages_cross_one_oversized_record_and_a_partial_tail() {
    const BOUNDARY: &str =
        "{\"timestamp\":\"2030-04-12T19:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n";
    let scratch = Scratch::new("codex-oversized-reverse-page");
    let transcript = scratch.transcript();
    let mut contents = BOUNDARY.to_owned();
    contents.push_str(&codex_assistant_record(0, &"x".repeat(100_000)));
    let complete = contents.len() as u64;
    contents.push_str("{\"timestamp\":\"2030-04-12T20:44:09.271Z\",\"type\":\"compacted\"");
    fs::write(&transcript, &contents).expect("partial session is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);
    cursor.initialized = false;

    let records = scan(&mut cursor).expect("large record makes backward progress");

    assert_eq!(
        cursor.startup_recovery.expect("completed boundary").offset,
        0
    );
    assert_eq!(records.len(), 1);
    assert_eq!(cursor.next, complete);
    append(&transcript, b",\"payload\":{}}\n");
    assert_eq!(
        scan(&mut cursor).expect("partial record completes").len(),
        1
    );
}

#[test]
fn incomplete_boundary_waits_for_its_jsonl_delimiter() {
    let scratch = Scratch::new("partial");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        b"{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"comp",
    )
    .expect("partial record is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    assert!(scan(&mut cursor).expect("partial scan succeeds").is_empty());
    assert_eq!(cursor.next, 0, "pending bytes never advance the cursor");
    append(&transcript, b"acted\",\"payload\":{}}\n");
    assert_eq!(scan(&mut cursor).expect("completed scan succeeds").len(), 1,);
}

#[test]
fn mixed_claude_calls_report_only_terminals_unavailable_at_each_boundary() {
    let scratch = Scratch::new("claude-unavailable");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"call-a\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}},",
            "{\"type\":\"tool_use\",\"id\":\"call-b\",\"name\":\"Read\",\"input\":{\"path\":\"Cargo.toml\"}}]}}\n",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"call-b\",\"content\":\"ok\",\"is_error\":false}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"call-c\",\"name\":\"Read\",\"input\":{\"path\":\"src/lib.rs\"}}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:10.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
        ),
    )
    .expect("mixed Claude transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);

    let mut records = scan(&mut cursor).expect("first Claude boundary scans");
    records.extend(scan(&mut cursor).expect("second Claude boundary scans"));
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["Bash: cargo test"],
        "the concrete Read result wins while the long command remains neutral",
    );
    assert_eq!(
        records[1].unavailable_tool_terminals,
        ["Read: src/lib.rs"],
        "the first boundary clears its context epoch",
    );
}

#[test]
fn codex_terminal_result_prevents_an_unavailable_marker() {
    let scratch = Scratch::new("codex-resolved");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"timestamp\":\"2030-04-12T18:44:08.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\",\"call_id\":\"call-a\"}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:08.500Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-a\",\"output\":\"Process exited with code 0\"}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n",
        ),
    )
    .expect("resolved Codex transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    let records = scan(&mut cursor).expect("resolved Codex transcript scans");
    assert_eq!(records.len(), 1);
    assert!(records[0].unavailable_tool_terminals.is_empty());
}

#[test]
fn codex_web_search_ends_resolve_only_their_own_calls_before_compaction() {
    let scratch = Scratch::new("codex-web-search-resolved");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\",\"call_id\":\"call-command\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"view_image\",\"arguments\":\"{\\\"path\\\":\\\"capture.png\\\"}\",\"call_id\":\"call-image\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"web_search_end\",\"call_id\":\"ws-without-companion\",\"query\":\"first\",\"action\":{\"type\":\"search\",\"query\":\"first\"}}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"web_search_call\",\"status\":\"completed\",\"action\":{\"type\":\"search\",\"query\":\"second\"}}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"web_search_end\",\"call_id\":\"ws-after-companion\",\"query\":\"second\",\"action\":{\"type\":\"search\",\"query\":\"second\"}}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-image\",\"output\":[{\"type\":\"image\",\"data\":\"iVBORw0KGgo=\",\"mimeType\":\"image/png\"}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\"}\n",
        ),
    )
    .expect("resolved web-search transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    let records = scan(&mut cursor).expect("resolved web-search transcript scans");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["exec_command: cargo test"],
        "missing and reordered search companions neither remain pending nor resolve neighboring calls",
    );
}

#[test]
fn typed_image_result_resolves_only_its_own_call_before_compaction() {
    let scratch = Scratch::new("codex-image-resolved");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"view_image\",\"arguments\":\"{\\\"path\\\":\\\"capture.png\\\"}\",\"call_id\":\"call-image\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\",\"call_id\":\"call-command\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-image\",\"output\":[{\"type\":\"image\",\"data\":\"iVBORw0KGgo=\",\"mimeType\":\"image/png\"}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\"}\n",
        ),
    )
    .expect("image-result transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    let records = scan(&mut cursor).expect("image-result transcript scans");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["exec_command: cargo test"],
        "media bytes neither become a command nor disturb the unrelated pending call",
    );
}

#[test]
fn claude_earlier_same_length_call_replacement_rebuilds_pending_calls() {
    const OLD_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"old\",\"name\":\"Read\",\"input\":{\"path\":\"old.rs\"}}]}}\n";
    const NEW_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"new\",\"name\":\"Read\",\"input\":{\"path\":\"new.rs\"}}]}}\n";
    const STABLE: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"stable final record\"}]}}\n";
    const BOUNDARY: &str = "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    assert_eq!(OLD_CALL.len(), NEW_CALL.len());
    let scratch = Scratch::new("claude-prefix-replacement");
    let transcript = scratch.transcript();
    let original = format!("{OLD_CALL}{STABLE}");
    let replacement = format!("{NEW_CALL}{STABLE}");
    assert_eq!(original.len(), replacement.len());
    fs::write(&transcript, &original).expect("initial transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    assert!(scan(&mut cursor).expect("initial call scans").is_empty());

    fs::write(&transcript, &replacement).expect("same-length replacement is written");
    assert_eq!(
        fs::metadata(&transcript).expect("metadata").len(),
        cursor.next
    );
    append(&transcript, BOUNDARY.as_bytes());

    let records = scan_until_boundary(&mut cursor, "replacement transcript rescans");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["Read: new.rs"]);
    assert_boundary_publishes_once(&transcript, listen::Dialect::Claude, records);
}

#[test]
fn codex_earlier_same_length_result_replacement_rebuilds_pending_calls() {
    const CALL: &str = "{\"timestamp\":\"2030-04-12T18:44:08.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\",\"call_id\":\"call-a\"}}\n";
    const RESULT_A: &str = "{\"timestamp\":\"2030-04-12T18:44:08.500Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-a\",\"output\":\"ok\"}}\n";
    const RESULT_B: &str = "{\"timestamp\":\"2030-04-12T18:44:08.500Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-b\",\"output\":\"ok\"}}\n";
    const STABLE: &str = "{\"timestamp\":\"2030-04-12T18:44:08.750Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"stable\"}]}}\n";
    const BOUNDARY: &str =
        "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\",\"payload\":{}}\n";
    assert_eq!(RESULT_A.len(), RESULT_B.len());
    let scratch = Scratch::new("codex-prefix-replacement");
    let transcript = scratch.transcript();
    let original = format!("{CALL}{RESULT_A}{STABLE}");
    let replacement = format!("{CALL}{RESULT_B}{STABLE}");
    assert_eq!(original.len(), replacement.len());
    fs::write(&transcript, &original).expect("resolved transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);
    assert!(scan(&mut cursor).expect("resolved prefix scans").is_empty());

    fs::write(&transcript, &replacement).expect("same-length result replacement is written");
    assert_eq!(
        fs::metadata(&transcript).expect("metadata").len(),
        cursor.next
    );
    append(&transcript, BOUNDARY.as_bytes());

    let records = scan_until_boundary(&mut cursor, "replacement transcript rescans");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["exec_command: cargo test"],
    );
    assert_boundary_publishes_once(&transcript, listen::Dialect::Codex, records);
}

mod history;

fn observed_state(lane: CanonicalUuid) -> MessageState {
    MessageState {
        previews: CallPreviews::default(),
        prose: BTreeMap::new(),
        observations: MessageObservations(vec![MessageObservation {
            lane,
            source: uuid("018f82d4-6f59-7d06-92e3-bce9131163bb"),
            from: wire::body::TranscriptPosition {
                member: 1,
                offset: 0,
            },
            to: wire::body::TranscriptPosition {
                member: 1,
                offset: 1,
            },
            at: observation_instant(),
        }]),
    }
}

fn observation_instant() -> wire::Instant {
    "2030-04-12T18:44:09.000Z"
        .parse()
        .expect("fixture observation instant")
}

fn codex_assistant_record(index: usize, filler: &str) -> String {
    format!(
        "{{\"timestamp\":\"2030-04-12T18:44:08.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"{index}:{filler}\"}}]}}}}\n"
    )
}

fn claude_environment(home: &Path) -> crate::session_command::Environment {
    crate::session_command::Environment::new(
        vec![crate::session_command::VendorEnvironment::new(
            &spice::extension::CLAUDE,
            None,
            Some(home.as_os_str().to_owned()),
        )],
        None,
        None,
    )
}

fn codex_environment(home: &Path) -> crate::session_command::Environment {
    crate::session_command::Environment::new(
        vec![crate::session_command::VendorEnvironment::new(
            &spice::extension::CODEX,
            None,
            Some(home.as_os_str().to_owned()),
        )],
        None,
        None,
    )
}

fn session_residency(
    lane: CanonicalUuid,
    author: CanonicalUuid,
    thread: &str,
) -> wire::ResidencyHead {
    session_residency_for(lane, author, thread, "claude")
}

fn session_residency_for(
    lane: CanonicalUuid,
    author: CanonicalUuid,
    thread: &str,
    vendor: &str,
) -> wire::ResidencyHead {
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163aa");
    wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree: "/tmp/session-compaction".to_owned(),
            life: "drive".to_owned(),
            vendor: vendor.parse().expect("fixture vendor"),
            occupant: Some(author),
            lineage: vec![author],
            capabilities: Vec::new(),
            session: Some(wire::LaneSession {
                lane,
                occupant: author,
                worktree: "/tmp/session-compaction".to_owned(),
                vendor: vendor.parse().expect("fixture vendor"),
                launch: None,
                epoch,
                stream: "stdout".to_owned(),
                identity: thread.to_owned(),
                locus: wire::TapeLocus {
                    member: 1,
                    offset: 0,
                    length: 1,
                },
            }),
            disposition: "occupied".to_owned(),
        }],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    }
}

fn cursor(path: &Path, dialect: listen::Dialect) -> Cursor {
    Cursor {
        thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
        path: path.to_path_buf(),
        worktree: PathBuf::from("/worktree"),
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
        startup_recovery: None,
    }
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
        publish(
            record,
            lane,
            author,
            Path::new("/worktree"),
            &publisher,
            &mut state,
        )
        .expect("boundary publishes");
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
        publish(
            record,
            lane,
            author,
            Path::new("/worktree"),
            &publisher,
            &mut state,
        )
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

fn large_claude_transcript(fill: char) -> String {
    let text = fill.to_string().repeat(4 * 1024);
    let record = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n",
    );
    format!(
        "{}{{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}}\n",
        record.repeat(96),
    )
}

fn claude_assistant_record(index: usize, filler: &str) -> String {
    format!(
        "{{\"type\":\"assistant\",\"message\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{index} {filler}\"}}]}}}}\n",
    )
}

fn large_claude_transcript_with_pending(fill: char, records: usize) -> String {
    const PENDING_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"held\",\"name\":\"Read\",\"input\":{\"path\":\"held.rs\"}}]}}\n";
    const BOUNDARY: &str = "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    let text = fill.to_string().repeat(4 * 1024);
    let record = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n",
    );
    format!("{}{PENDING_CALL}{BOUNDARY}", record.repeat(records))
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}
