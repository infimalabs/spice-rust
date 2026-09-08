use super::*;

#[test]
fn unchanged_append_only_prefix_never_replays_an_earlier_boundary() {
    let scratch = Scratch::new("append-only");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
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
        b"{\"timestamp\":\"2030-04-12T18:44:10.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    );
    let records = scan(&mut cursor).expect("later boundary scans");
    assert_eq!(records.len(), 1, "the earlier boundary is not replayed");
    assert_eq!(records[0].unavailable_tool_terminals, ["Read: later.rs"]);
}
#[test]
fn resident_reload_bootstraps_only_boundaries_in_the_initial_visible_tail() {
    let scratch = Scratch::new("retained-tail-bootstrap");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut contents = String::new();
    for index in 0..300 {
        contents.push_str(&claude_assistant_record(index, &filler));
        if matches!(index, 80 | 180 | 260 | 285) {
            contents.push_str(
                "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
            );
        }
    }
    fs::write(&transcript, &contents).expect("long preserved transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    cursor.initialized = false;

    let records = scan(&mut cursor).expect("resident tail bootstrap scans");

    assert_eq!(
        records.len(),
        1,
        "only the initial-tail boundary is admitted"
    );
    assert!(cursor.initialized);
    assert_eq!(
        cursor.next,
        u64::try_from(contents.len()).expect("fixture length fits u64"),
        "bootstrap continues at the complete durable tail",
    );
    assert_eq!(cursor.prefix_start, cursor.next);
    assert!(cursor.validation.is_none());
    assert!(
        records.windows(2).all(|pair| {
            let earlier = pair[0].decoded.locus();
            let later = pair[1].decoded.locus();
            (earlier.member, earlier.offset, earlier.length)
                < (later.member, later.offset, later.length)
        }),
        "tail boundaries preserve source chronology",
    );
}

#[test]
fn on_demand_history_recovers_boundaries_across_the_retained_horizon() {
    let scratch = Scratch::new("retained-history");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut contents = String::new();
    for index in 0..300 {
        contents.push_str(&claude_assistant_record(index, &filler));
        if matches!(index, 80 | 180 | 260 | 285) {
            contents.push_str(
                "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
            );
        }
    }
    fs::write(&transcript, contents).expect("long preserved transcript is written");
    let end = history_checkpoint(&transcript).expect("preserved transcript checkpoint reads");
    let before = wire::AgentMessage {
        lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        locus: wire::MessageLocus {
            member: session_member("018f7777-7777-7444-8888-999999999999"),
            offset: u64::MAX,
            length: 1,
            event: 0,
        },
        authored: "2031-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        authored_synthetic: false,
        observed: None,
        activity: wire::AgentActivityKind::Prose,
        final_answer: false,
        text: "page boundary".to_owned(),
        badges: Default::default(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        media: Vec::new(),
    };

    let messages = retained_messages(HistoryRequest {
        path: transcript,
        worktree: PathBuf::from("/worktree"),
        dialect: listen::Dialect::Claude,
        thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
        lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        observations: &MessageObservations::default(),
        before: &before,
        end: &end,
        limit: canvas::stream::MESSAGE_CAPACITY,
    })
    .expect("retained compaction history reads");

    assert_eq!(messages.messages.len(), 4);
    assert!(
        messages
            .messages
            .windows(2)
            .all(|pair| pair[0].chronology_key() < pair[1].chronology_key()),
        "on-demand boundaries preserve source chronology",
    );
}

#[test]
fn resident_reload_does_not_manufacture_a_boundary_from_a_long_prose_tail() {
    let scratch = Scratch::new("prose-only-tail-bootstrap");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let contents = (0..300)
        .map(|index| claude_assistant_record(index, &filler))
        .collect::<String>();
    fs::write(&transcript, &contents).expect("long prose transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    cursor.initialized = false;

    assert!(
        scan(&mut cursor)
            .expect("prose-only tail bootstrap scans")
            .is_empty(),
        "only typed durable compactions create boundaries",
    );
    assert!(cursor.initialized);
    assert_eq!(cursor.prefix_start, cursor.next);
}

#[test]
fn live_boundary_after_tail_bootstrap_authenticates_only_the_new_context_epoch() {
    const CALL: &str = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"later\",\"name\":\"Read\",\"input\":{\"path\":\"later.rs\"}}]}}\n";
    const BOUNDARY: &str = "{\"timestamp\":\"2030-04-12T19:44:10.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    let scratch = Scratch::new("post-bootstrap-live-boundary");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let contents = (0..300)
        .map(|index| claude_assistant_record(index, &filler))
        .collect::<String>();
    fs::write(&transcript, &contents).expect("long prose transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    cursor.initialized = false;
    assert!(scan(&mut cursor).expect("tail bootstrap scans").is_empty());
    let epoch_start = cursor.next;
    append(&transcript, format!("{CALL}{BOUNDARY}").as_bytes());

    reset_validation_bytes_read();
    let records = scan(&mut cursor).expect("live boundary scans");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["Read: later.rs"]);
    assert_eq!(cursor.prefix_start, cursor.next);
    assert!(cursor.prefix_start > epoch_start);
    assert!(
        validation_bytes_read()
            <= u64::try_from(CALL.len() + BOUNDARY.len()).expect("suffix length fits u64"),
        "the live boundary does not re-authenticate the long preserved prefix",
    );
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
    const LATER_BOUNDARY: &str = "{\"timestamp\":\"2030-04-12T18:44:10.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
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
fn timestamped_claude_boundary_uses_the_same_typed_projection() {
    let scratch = Scratch::new("claude");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        b"{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    )
    .expect("Claude boundary is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    let records = scan(&mut cursor).expect("Claude session scans");

    assert_eq!(records.len(), 1);
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
    .expect("timestamped Claude boundary publishes");
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
}

#[test]
fn timestamped_claude_boundary_survives_live_poll_and_resident_reload_once() {
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
        .synchronize_display_only(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("initial synchronization remains live before a boundary");
    append(
        &transcript,
        concat!(
            "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"call-live\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
        )
        .as_bytes(),
    );
    compactions
        .synchronize_display_only(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("live polling publishes the timestamped boundary");
    compactions
        .synchronize_display_only(&residency, &publisher, &mut state, &mut diagnostic)
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
        .synchronize_display_only(&residency, &publisher, &mut reloaded_state, &mut diagnostic)
        .expect("resident reload replays the same durable identity without fault");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1, "poll and reload retain one boundary");
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert!(!messages[0].authored_synthetic);
    assert_eq!(
        messages[0].unavailable_tool_terminals,
        ["Bash: cargo test"],
        "reload replays the same boundary-bound unavailable terminal",
    );
    assert_eq!(
        messages[0].observed, None,
        "the transcript timestamp is the only compaction chronology source",
    );
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
}

#[test]
fn timestamp_less_claude_boundary_is_refused_without_a_chronology_fallback() {
    const THREAD: &str = "018f7777-7777-7444-8888-999999999999";

    let scratch = Scratch::new("claude-missing-time");
    let transcript = scratch
        .0
        .join("projects")
        .join(format!("session-{THREAD}.jsonl"));
    fs::create_dir_all(transcript.parent().expect("transcript parent"))
        .expect("Claude project directory exists");
    fs::write(
        &transcript,
        concat!(
            "{\"type\":\"assistant\",\"message\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"tool_use\",\"id\":\"stale-call\",\"name\":\"Read\",\"input\":{\"path\":\"stale.rs\"}}]}}\n",
            "{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
        ),
    )
    .expect("timestamp-less Claude boundary is written");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let author = uuid("018f82d4-6f59-7d06-92e3-bce9131163ef");
    let publisher = Messages::default();
    let mut state = observed_state(lane);
    let mut diagnostic = Vec::new();
    let mut compactions = SessionCompactions::new(claude_environment(&scratch.0));

    compactions
        .synchronize_display_only(
            &session_residency(lane, author, THREAD),
            &publisher,
            &mut state,
            &mut diagnostic,
        )
        .expect("a malformed preserved boundary does not stop the resident");

    assert!(publisher.0.lock().expect("message lock").is_empty());
    assert!(
        String::from_utf8(diagnostic.clone())
            .expect("diagnostic is UTF-8")
            .contains("preserved session compaction record carries no timestamp"),
        "the missing contract field is diagnosed without another chronology source",
    );

    append(
        &transcript,
        b"{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    );
    compactions
        .synchronize_display_only(
            &session_residency(lane, author, THREAD),
            &publisher,
            &mut state,
            &mut diagnostic,
        )
        .expect("a later valid boundary remains publishable");
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert!(
        messages[0].unavailable_tool_terminals.is_empty(),
        "the refused boundary clears its unavailable-tool epoch",
    );
}
