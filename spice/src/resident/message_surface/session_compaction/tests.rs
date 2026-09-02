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
    assert_eq!(
        records[0].authored.as_deref(),
        Some("2030-04-12T18:44:09.271Z")
    );
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
            "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"call-c\",\"name\":\"Read\",\"input\":{\"path\":\"src/lib.rs\"}}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
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
fn claude_earlier_same_length_call_replacement_rebuilds_pending_calls() {
    const OLD_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"old\",\"name\":\"Read\",\"input\":{\"path\":\"old.rs\"}}]}}\n";
    const NEW_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"new\",\"name\":\"Read\",\"input\":{\"path\":\"new.rs\"}}]}}\n";
    const STABLE: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"stable final record\"}]}}\n";
    const BOUNDARY: &str = "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
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
        ["exec command: cargo test"],
    );
    assert_boundary_publishes_once(&transcript, listen::Dialect::Codex, records);
}

#[test]
fn unchanged_append_only_prefix_never_replays_an_earlier_boundary() {
    let scratch = Scratch::new("append-only");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    )
    .expect("initial boundary is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    assert_eq!(scan(&mut cursor).expect("initial scan").len(), 1);

    append(
        &transcript,
        b"{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"later\",\"name\":\"Read\",\"input\":{\"path\":\"later.rs\"}}]}}\n",
    );
    assert!(scan(&mut cursor).expect("call append scans").is_empty());
    append(
        &transcript,
        b"{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    );
    let records = scan(&mut cursor).expect("later boundary scans");
    assert_eq!(records.len(), 1, "the earlier boundary is not replayed");
    assert_eq!(records[0].unavailable_tool_terminals, ["Read: later.rs"]);
}

#[test]
fn large_prefix_validation_is_bounded_and_restarts_after_generation_movement() {
    let scratch = Scratch::new("progressive-validation");
    let transcript = scratch.transcript();
    let original = large_claude_transcript('x');
    let replacement = large_claude_transcript('y');
    assert_eq!(original.len(), replacement.len());
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);

    assert!(scan(&mut cursor).expect("first bounded scan").is_empty());
    assert!(
        cursor.validation.is_none(),
        "the byte budget precedes the boundary"
    );
    assert!(
        cursor.next <= (SCAN_BYTE_BUDGET + 8 * 1024) as u64,
        "one complete record is the only permitted soft-budget overrun",
    );
    assert!(scan(&mut cursor).expect("second bounded scan").is_empty());
    let validation = cursor
        .validation
        .as_ref()
        .expect("boundary validation is pending");
    assert_eq!(validation.offset, VALIDATION_BYTE_BUDGET as u64);
    assert!(validation.length > validation.offset);

    let staged = scratch.0.join("replacement.jsonl");
    fs::write(&staged, replacement).expect("replacement generation is staged");
    fs::rename(staged, &transcript).expect("replacement generation is published");
    assert!(
        scan(&mut cursor)
            .expect("moved generation refuses")
            .is_empty()
    );
    assert_eq!(cursor.next, 0, "mixed-generation state is discarded");
    assert!(cursor.validation.is_none());

    let records = scan_until_boundary(&mut cursor, "replacement generation replays");
    assert_eq!(records.len(), 1);
    assert!(records[0].unavailable_tool_terminals.is_empty());
}

#[test]
fn append_only_growth_preserves_progress_and_stays_behind_the_held_boundary() {
    const LATER_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"later\",\"name\":\"Read\",\"input\":{\"path\":\"later.rs\"}}]}}\n";
    const LATER_BOUNDARY: &str = "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    let scratch = Scratch::new("append-during-validation");
    let transcript = scratch.transcript();
    fs::write(&transcript, large_claude_transcript_with_pending('x', 192))
        .expect("large active transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);

    for _ in 0..8 {
        assert!(scan(&mut cursor).expect("prefix scans").is_empty());
        if cursor.validation.is_some() {
            break;
        }
    }
    let validation = cursor
        .validation
        .as_ref()
        .expect("large boundary validation is pending");
    assert_eq!(validation.offset, VALIDATION_BYTE_BUDGET as u64);
    assert_eq!(
        validation.records[0].unavailable_tool_terminals,
        ["Read: held.rs"]
    );
    let frozen_next = cursor.next;
    let frozen_next_call = cursor.next_call;

    let mut records = Vec::new();
    let mut validation_reads = 0_u64;
    for serial in 0..8 {
        let suffix = if serial == 0 {
            LATER_CALL.to_owned()
        } else {
            format!("{{\"type\":\"progress\",\"serial\":{serial}}}\n")
        };
        append(&transcript, suffix.as_bytes());
        let previous = cursor
            .validation
            .as_ref()
            .expect("validation remains pending before the poll")
            .offset;
        reset_validation_bytes_read();
        records = scan(&mut cursor).expect("growing transcript validates");
        let read = validation_bytes_read();
        validation_reads = validation_reads.saturating_add(read);
        assert!(
            read <= (VALIDATION_BYTE_BUDGET * 3) as u64,
            "one growing poll stays inside the fixed authentication and validation bound",
        );
        assert_eq!(cursor.next, frozen_next, "suffix remains unconsumed");
        assert_eq!(cursor.next_call, frozen_next_call);
        assert!(
            records.is_empty(),
            "active growth waits for stable authority"
        );
        let validation = cursor
            .validation
            .as_ref()
            .expect("validation remains pending");
        assert!(
            validation.offset >= previous,
            "append-only growth never loses primary validation progress",
        );
    }
    let growing = cursor
        .validation
        .as_ref()
        .expect("active growth retains the held boundary");
    assert_eq!(
        growing.offset, growing.length,
        "primary validation reaches the frozen prefix during active growth",
    );
    assert!(
        growing.authentication.is_some(),
        "publication waits for one generation-stable authentication pass",
    );
    for _ in 0..8 {
        reset_validation_bytes_read();
        records = scan(&mut cursor).expect("stable transcript validates");
        let read = validation_bytes_read();
        validation_reads = validation_reads.saturating_add(read);
        assert!(
            read <= (VALIDATION_BYTE_BUDGET * 3) as u64,
            "one stable poll stays inside the fixed authentication and validation bound",
        );
        if !records.is_empty() {
            break;
        }
    }
    assert_eq!(records.len(), 1, "the held boundary eventually publishes");
    assert!(
        validation_reads <= (VALIDATION_BYTE_BUDGET * 3 * 16) as u64,
        "repeated growth has a linear cumulative read bound",
    );
    assert_eq!(
        records[0].unavailable_tool_terminals,
        ["Read: held.rs"],
        "the appended call cannot enter the held boundary",
    );

    append(&transcript, LATER_BOUNDARY.as_bytes());
    let later = scan_until_boundary(&mut cursor, "appended records scan after validation");
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].unavailable_tool_terminals, ["Read: later.rs"]);
    assert!(scan(&mut cursor).expect("settled poll scans").is_empty());
}

#[test]
fn same_length_mutation_after_admitted_growth_resets_validation() {
    const SUFFIX: &str = "{\"type\":\"progress\",\"serial\":1}\n";
    let scratch = Scratch::new("mutation-after-append");
    let transcript = scratch.transcript();
    fs::write(&transcript, large_claude_transcript_with_pending('x', 192))
        .expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    for _ in 0..8 {
        assert!(scan(&mut cursor).expect("prefix scans").is_empty());
        if cursor.validation.is_some() {
            break;
        }
    }
    append(&transcript, SUFFIX.as_bytes());
    assert!(
        scan(&mut cursor)
            .expect("append growth validates")
            .is_empty()
    );
    let advanced = cursor
        .validation
        .as_ref()
        .expect("validation remains pending")
        .offset;

    let replacement = format!("{}{SUFFIX}", large_claude_transcript_with_pending('y', 192));
    fs::write(&transcript, replacement).expect("same-length mutation is written in place");
    assert!(scan(&mut cursor).expect("mutation refuses").is_empty());
    assert_eq!(cursor.next, 0, "mixed-generation scan state is discarded");
    assert!(cursor.validation.is_none());
    assert!(advanced > VALIDATION_BYTE_BUDGET as u64);
}

#[test]
fn prefix_mutation_hidden_by_growth_resets_validation() {
    const SUFFIX: &str = "{\"type\":\"progress\",\"serial\":1}\n";
    let scratch = Scratch::new("mutation-with-append");
    let transcript = scratch.transcript();
    let original = large_claude_transcript_with_pending('x', 192);
    let mutation = u64::try_from(original.find("xxxx").expect("fixture text exists"))
        .expect("fixture offset fits u64");
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    for _ in 0..8 {
        assert!(scan(&mut cursor).expect("prefix scans").is_empty());
        if cursor.validation.is_some() {
            break;
        }
    }
    append(&transcript, SUFFIX.as_bytes());
    assert!(scan(&mut cursor).expect("append advances").is_empty());
    let advanced = cursor
        .validation
        .as_ref()
        .expect("validation remains pending")
        .offset;
    assert!(mutation < advanced);

    let mut rewritten = OpenOptions::new()
        .write(true)
        .open(&transcript)
        .expect("transcript opens for a prefix rewrite");
    rewritten
        .seek(std::io::SeekFrom::Start(mutation))
        .expect("prefix mutation seeks");
    rewritten.write_all(b"y").expect("prefix mutation writes");
    drop(rewritten);
    append(&transcript, SUFFIX.as_bytes());

    assert!(scan(&mut cursor).expect("mixed growth refuses").is_empty());
    assert_eq!(cursor.next, 0, "mixed-generation scan state is discarded");
    assert_eq!(cursor.next_call, 0);
    assert!(cursor.pending.is_empty());
    assert!(cursor.validation.is_none());

    let records = scan_until_boundary(&mut cursor, "rewritten generation replays");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["Read: held.rs"]);
}

#[test]
fn mutation_during_extension_authentication_discards_the_moving_slice() {
    const SUFFIX: &str = "{\"type\":\"progress\",\"serial\":1}\n";
    let scratch = Scratch::new("mutation-during-authentication");
    let transcript = scratch.transcript();
    let original = large_claude_transcript_with_pending('x', 192);
    let mutation = u64::try_from(original.find("xxxx").expect("fixture text exists"))
        .expect("fixture offset fits u64");
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    for _ in 0..8 {
        assert!(scan(&mut cursor).expect("prefix scans").is_empty());
        if cursor.validation.is_some() {
            break;
        }
    }
    let frozen = cursor
        .validation
        .as_ref()
        .expect("validation is pending")
        .offset;
    append(&transcript, SUFFIX.as_bytes());
    let moving = transcript.clone();
    cursor
        .validation
        .as_mut()
        .expect("validation remains pending")
        .after_authentication_read = Some(Box::new(move || {
        let mut rewritten = OpenOptions::new()
            .write(true)
            .open(&moving)
            .expect("transcript opens for an in-flight rewrite");
        rewritten
            .seek(std::io::SeekFrom::Start(mutation))
            .expect("in-flight mutation seeks");
        rewritten
            .write_all(b"y")
            .expect("in-flight mutation writes");
        drop(rewritten);
        append(&moving, SUFFIX.as_bytes());
    }));

    reset_validation_bytes_read();
    assert!(
        scan(&mut cursor)
            .expect("moving authentication defers")
            .is_empty()
    );
    assert!(validation_bytes_read() <= VALIDATION_BYTE_BUDGET as u64);
    let validation = cursor
        .validation
        .as_ref()
        .expect("the uncommitted authentication keeps original scan state");
    assert_eq!(validation.offset, frozen);
    assert_eq!(
        validation
            .authentication
            .as_ref()
            .expect("moved slice restarts authentication")
            .offset,
        0,
    );

    assert!(
        scan(&mut cursor)
            .expect("rewritten prefix refuses")
            .is_empty()
    );
    assert_eq!(cursor.next, 0, "mixed-generation scan state is discarded");
    assert_eq!(cursor.next_call, 0);
    assert!(cursor.pending.is_empty());
    assert!(cursor.validation.is_none());
}

#[test]
fn rewrite_between_authentication_slices_cannot_publish_stale_records() {
    const SUFFIX: &str = "{\"type\":\"progress\",\"serial\":1}\n";
    let scratch = Scratch::new("mutation-between-authentication-slices");
    let transcript = scratch.transcript();
    let original = large_claude_transcript_with_pending('x', 192);
    let mutation = u64::try_from(original.find("xxxx").expect("fixture text exists"))
        .expect("fixture offset fits u64");
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    for _ in 0..8 {
        assert!(scan(&mut cursor).expect("prefix scans").is_empty());
        if cursor
            .validation
            .as_ref()
            .is_some_and(|validation| validation.offset > AUTHENTICATION_BYTE_BUDGET as u64)
        {
            break;
        }
    }
    assert!(
        cursor
            .validation
            .as_ref()
            .is_some_and(|validation| { validation.offset > AUTHENTICATION_BYTE_BUDGET as u64 }),
        "fixture reaches a multi-slice authentication prefix",
    );

    append(&transcript, SUFFIX.as_bytes());
    assert!(
        scan(&mut cursor)
            .expect("first authentication slice")
            .is_empty()
    );
    let authentication = cursor
        .validation
        .as_ref()
        .expect("validation remains pending")
        .authentication
        .as_ref()
        .expect("authentication spans polls");
    assert!(mutation < authentication.offset);

    let mut rewritten = OpenOptions::new()
        .write(true)
        .open(&transcript)
        .expect("transcript opens for a between-slice rewrite");
    rewritten
        .seek(std::io::SeekFrom::Start(mutation))
        .expect("between-slice mutation seeks");
    rewritten
        .write_all(b"y")
        .expect("between-slice mutation writes");
    drop(rewritten);
    append(&transcript, SUFFIX.as_bytes());

    reset_validation_bytes_read();
    assert!(
        scan(&mut cursor)
            .expect("between-slice rewrite defers")
            .is_empty(),
        "a stale authenticated slice published held records",
    );
    assert!(validation_bytes_read() <= VALIDATION_BYTE_BUDGET as u64);
    let validation = cursor
        .validation
        .as_ref()
        .expect("the held boundary remains unpublished");
    assert_eq!(
        validation
            .authentication
            .as_ref()
            .expect("new extent restarts authentication")
            .offset,
        0,
    );

    for _ in 0..8 {
        assert!(
            scan(&mut cursor)
                .expect("rewritten prefix refuses")
                .is_empty(),
            "mixed held records never publish while the rewritten prefix is checked",
        );
        if cursor.validation.is_none() {
            break;
        }
    }
    assert_eq!(cursor.next, 0, "mixed-generation scan state is discarded");
    assert_eq!(cursor.next_call, 0);
    assert!(cursor.pending.is_empty());
    assert!(cursor.validation.is_none());

    let records = scan_until_boundary(&mut cursor, "rewritten generation replays");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["Read: held.rs"]);
}

#[test]
fn claude_preserved_boundary_uses_the_same_typed_projection() {
    let scratch = Scratch::new("claude");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        b"{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    )
    .expect("Claude boundary is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    let records = scan(&mut cursor).expect("Claude session scans");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].authored, None);
    assert!(matches!(records[0].decoded, listen::Event::Compaction(_)));

    let publisher = Messages::default();
    publish(
        records.into_iter().next().expect("one compaction"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        &publisher,
        &mut MessageState::default(),
    )
    .expect("timestamp-less Claude boundary publishes");
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert!(messages[0].authored_synthetic);
    assert!(messages[0].unavailable_tool_terminals.is_empty());
    assert_eq!(
        messages[0].locus.member,
        session_member("018f7777-7777-7444-8888-999999999999"),
        "synthetic chronology does not erase the preserved-session identity",
    );
    assert_eq!(
        messages[0].authored,
        "2000-01-01T00:00:00.000Z"
            .parse::<wire::Instant>()
            .expect("session synthetic authored instant parses"),
    );
}

#[test]
fn timestamp_less_claude_boundary_survives_live_poll_and_resident_reload_once() {
    const THREAD: &str = "018f7777-7777-7444-8888-999999999999";

    let scratch = Scratch::new("claude-synchronize");
    let transcript = scratch
        .0
        .join("projects")
        .join(format!("session-{THREAD}.jsonl"));
    fs::create_dir_all(transcript.parent().expect("transcript parent"))
        .expect("Claude project directory exists");
    fs::write(&transcript, b"").expect("empty Claude session exists at initial sync");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let author = uuid("018f82d4-6f59-7d06-92e3-bce9131163ef");
    let residency = session_residency(lane, author, THREAD);
    let publisher = Messages::default();
    let mut state = observed_state(lane);
    let mut diagnostic = Vec::new();
    let mut compactions = SessionCompactions::new(claude_environment(&scratch.0));

    compactions
        .synchronize(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("initial synchronization remains live before a boundary");
    append(
        &transcript,
        concat!(
            "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"call-live\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
        )
        .as_bytes(),
    );
    compactions
        .synchronize(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("live polling publishes the timestamp-less boundary");
    compactions
        .synchronize(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("an unchanged live poll remains quiet");

    let mut reloaded = SessionCompactions::new(claude_environment(&scratch.0));
    let mut reloaded_state = observed_state(lane);
    reloaded_state.observations.0.push(MessageObservation {
        lane,
        source: uuid("018f82d4-6f59-7d06-92e3-bce9131163bc"),
        from: wire::body::TranscriptPosition {
            member: 2,
            offset: 0,
        },
        to: wire::body::TranscriptPosition {
            member: 2,
            offset: 1,
        },
        at: "2031-04-12T18:44:09.000Z"
            .parse()
            .expect("later observation instant"),
    });
    reloaded
        .synchronize(&residency, &publisher, &mut reloaded_state, &mut diagnostic)
        .expect("resident reload replays the same durable identity without fault");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1, "poll and reload retain one boundary");
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert!(messages[0].authored_synthetic);
    assert_eq!(
        messages[0].unavailable_tool_terminals,
        ["Bash: cargo test"],
        "reload replays the same boundary-bound unavailable terminal",
    );
    assert_eq!(
        messages[0].observed,
        Some(observation_instant()),
        "the durable lane observation places the late session boundary",
    );
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
}

fn observed_state(lane: CanonicalUuid) -> MessageState {
    MessageState {
        previews: CallPreviews::default(),
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

fn session_residency(
    lane: CanonicalUuid,
    author: CanonicalUuid,
    thread: &str,
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
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(author),
            lineage: vec![author],
            capabilities: Vec::new(),
            session: Some(wire::LaneSession {
                lane,
                occupant: author,
                worktree: "/tmp/session-compaction".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
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
    }
}

fn cursor(path: &Path, dialect: listen::Dialect) -> Cursor {
    Cursor {
        thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
        path: path.to_path_buf(),
        dialect,
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        member: session_member("018f7777-7777-7444-8888-999999999999"),
        observed: None,
        next: 0,
        next_call: 0,
        pending: BTreeMap::new(),
        prefix: blake3::Hasher::new(),
        guard: None,
        validation: None,
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

fn large_claude_transcript(fill: char) -> String {
    let text = fill.to_string().repeat(4 * 1024);
    let record = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n",
    );
    format!(
        "{}{{\"type\":\"system\",\"subtype\":\"compact_boundary\"}}\n",
        record.repeat(96),
    )
}

fn large_claude_transcript_with_pending(fill: char, records: usize) -> String {
    const PENDING_CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"held\",\"name\":\"Read\",\"input\":{\"path\":\"held.rs\"}}]}}\n";
    const BOUNDARY: &str = "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    let text = fill.to_string().repeat(4 * 1024);
    let record = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n",
    );
    format!("{}{PENDING_CALL}{BOUNDARY}", record.repeat(records))
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}
