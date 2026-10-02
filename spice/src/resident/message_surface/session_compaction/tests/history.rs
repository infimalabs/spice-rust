use super::*;

#[test]
fn unchanged_append_only_prefix_never_replays_an_earlier_boundary() {
    let scratch = Scratch::new("append-only");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n",
    )
    .expect("initial boundary is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    assert_eq!(scan(&mut cursor).expect("initial scan").len(), 1);

    append(
        &transcript,
        b"{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"later\",\"name\":\"read\",\"arguments\":{\"path\":\"later.rs\"}}]}}\n",
    );
    assert!(scan(&mut cursor).expect("call append scans").is_empty());
    append(
        &transcript,
        b"{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:10.271Z\"}\n",
    );
    let records = scan(&mut cursor).expect("later boundary scans");
    assert_eq!(records.len(), 1, "the earlier boundary is not replayed");
    assert_eq!(records[0].unavailable_tool_terminals, ["read: later.rs"]);
}
#[test]
fn resident_reload_bootstraps_only_boundaries_in_the_initial_visible_tail() {
    let scratch = Scratch::new("retained-tail-bootstrap");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut contents = String::new();
    for index in 0..300 {
        contents.push_str(&assistant_record(index, &filler));
        if matches!(index, 80 | 180 | 260 | 285) {
            contents
                .push_str("{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n");
        }
    }
    fs::write(&transcript, &contents).expect("long preserved transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    cursor.initialized = false;

    let records = bootstrap_until_ready(&mut cursor, "resident tail bootstrap scans");

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

fn page_boundary_marker() -> wire::AgentMessage {
    wire::AgentMessage {
        prose_origin: None,
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
        provider_error: None,
        classifications: Vec::new(),
        quote_target: None,
        directive_contexts: Vec::new(),
        quotes: Vec::new(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        boundary: None,
        media: Vec::new(),
    }
}

fn retained_history_request<'a>(
    transcript: &Path,
    before: &'a wire::AgentMessage,
    end: &'a HistoryCheckpoint,
) -> HistoryRequest<'a> {
    HistoryRequest {
        path: transcript.to_path_buf(),
        dialect: listen::Dialect::Omp,
        thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
        lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        before: Some(before),
        end,
        limit: serve::stream::MESSAGE_CAPACITY,
    }
}

/// Windows the retained horizon fixture outgrows.
const RETAINED_HORIZON_WINDOWS: usize = 2;
/// One preserved record wider than a history window.
const OVERSIZED_RECORD_BYTES: usize = 300 * 1024;
/// One preserved record wider than two history windows.
const WIDE_RECORD_BYTES: usize = 600 * 1024;

fn write_retained_horizon(transcript: &PathBuf) -> HistoryCheckpoint {
    let filler = "x".repeat(1_800);
    let mut contents = String::new();
    for index in 0..300 {
        contents.push_str(&assistant_record(index, &filler));
        if matches!(index, 80 | 180 | 260 | 285) {
            contents
                .push_str("{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n");
        }
    }
    assert!(
        contents.len() > RETAINED_HORIZON_WINDOWS * HISTORY_BYTE_BUDGET,
        "the retained horizon outgrows two history windows",
    );
    fs::write(transcript, contents).expect("long preserved transcript is written");
    history_checkpoint(transcript).expect("preserved transcript checkpoint reads")
}

#[test]
fn on_demand_history_recovers_boundaries_across_the_retained_horizon() {
    let scratch = Scratch::new("retained-history");
    let transcript = scratch.transcript();
    let end = write_retained_horizon(&transcript);
    let before = page_boundary_marker();

    let messages = (0..8)
        .find_map(|_| {
            let page = retained_messages(
                retained_history_request(&transcript, &before, &end),
                &mut SessionCards::default(),
            )
            .expect("retained compaction history reads");
            (!page.messages.is_empty()).then_some(page)
        })
        .expect("retained authentication settles before the page is requested");

    assert_eq!(
        messages.messages.len(),
        4,
        "one request chains to the source start"
    );
    assert!(!messages.more);
    assert_eq!(messages.start, 0);
    assert!(
        !messages.work.within_limits(),
        "the chained request spends more than one window allowance",
    );
    assert!(messages.work.passes > HISTORY_PASS_BUDGET);
    assert!(messages.work.source_bytes > HISTORY_BYTE_BUDGET as u64);
    assert!(
        messages
            .messages
            .windows(2)
            .all(|pair| pair[0].chronology_key() < pair[1].chronology_key()),
        "on-demand boundaries preserve source chronology",
    );
}

#[test]
fn consumers_at_different_anchors_share_one_decoded_suffix() {
    let scratch = Scratch::new("retained-history-shared");
    let transcript = scratch.transcript();
    let end = write_retained_horizon(&transcript);
    let marker = page_boundary_marker();
    let mut cards = SessionCards::default();
    let newest = (0..8)
        .find_map(|_| {
            let mut request = retained_history_request(&transcript, &marker, &end);
            request.limit = 1;
            let page = retained_messages(request, &mut cards).expect("newest boundary reads");
            (!page.messages.is_empty()).then_some(page)
        })
        .expect("retained authentication settles before the page is requested");
    assert_eq!(newest.messages.len(), 1);
    assert!(newest.more);

    let deep = retained_messages(
        retained_history_request(&transcript, &marker, &end),
        &mut cards,
    )
    .expect("a deeper consumer extends the shared suffix");
    assert_eq!(deep.messages.len(), 4);
    assert!(!deep.more);

    let behind = &newest.messages[0];
    let shallow = retained_messages(
        retained_history_request(&transcript, behind, &end),
        &mut cards,
    )
    .expect("a consumer behind the newest boundary reads");
    assert_eq!(
        shallow.work.source_bytes, 0,
        "a region another consumer decoded is not decoded again",
    );
    assert_eq!(
        shallow.messages,
        deep.messages[..3],
        "each consumer selects its own page before its own anchor",
    );
    assert!(!shallow.more);
}

#[test]
fn each_history_window_stays_inside_its_fixed_allowances() {
    let scratch = Scratch::new("retained-history-window");
    let transcript = scratch.transcript();
    let end = write_retained_horizon(&transcript);
    let before = page_boundary_marker();
    let request = retained_history_request(&transcript, &before, &end);
    let mut cursor = Cursor::new(
        request.thread.clone(),
        request.path.clone(),
        request.dialect,
        request.author,
    );

    let newest = (0..8)
        .find_map(|_| {
            let window = history_window(&mut cursor, &request, request.end)
                .expect("newest history window reads");
            (!window.messages.is_empty()).then_some(window)
        })
        .expect("retained authentication settles before the window is requested");

    assert_eq!(
        newest.messages.len(),
        3,
        "one window holds the newest three boundaries"
    );
    assert!(newest.more);
    assert!(newest.start > 0);
    assert!(newest.work.within_limits());
    assert_eq!(newest.work.passes, HISTORY_PASS_BUDGET);
    assert!(newest.work.source_bytes <= HISTORY_BYTE_BUDGET as u64);
    assert!(newest.work.decoded_records <= HISTORY_RECORD_BUDGET as u32);
    let older_end = end.retreat(newest.start);
    let older =
        history_window(&mut cursor, &request, &older_end).expect("older history window reads");
    assert_eq!(older.messages.len(), 1);
    assert!(older.work.within_limits());
    assert_eq!(older.work.passes, HISTORY_PASS_BUDGET);
    assert!(older.work.source_bytes <= HISTORY_BYTE_BUDGET as u64);
}

/// Write two compaction boundaries around one assistant record of `record_bytes`
/// text with a short filler run on either side, and serve one history page.
fn page_around_oversized_record(scratch: &Scratch, record_bytes: usize) -> HistoryScan {
    const EARLIER: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n";
    const LATER: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T19:44:09.271Z\"}\n";
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut contents = String::from(EARLIER);
    contents.extend((0..20).map(|index| assistant_record(index, &filler)));
    contents.push_str(&assistant_record(20, &"y".repeat(record_bytes)));
    contents.extend((21..41).map(|index| assistant_record(index, &filler)));
    contents.push_str(LATER);
    fs::write(&transcript, contents).expect("transcript with an oversized record is written");
    let end = history_checkpoint(&transcript).expect("oversized transcript checkpoint reads");
    let before = page_boundary_marker();
    (0..8)
        .find_map(|_| {
            let page = retained_messages(
                retained_history_request(&transcript, &before, &end),
                &mut SessionCards::default(),
            )
            .expect("the page is served around the oversized record");
            (!page.messages.is_empty()).then_some(page)
        })
        .expect("retained authentication settles before the page is requested")
}

fn assert_both_boundaries_served(page: &HistoryScan) {
    assert_eq!(
        page.messages.len(),
        2,
        "both boundaries are served around the skipped record"
    );
    assert!(
        page.messages
            .iter()
            .all(|message| message.activity == wire::AgentActivityKind::Compaction),
        "only compaction cards are served",
    );
    assert!(page.messages[0].chronology_key() < page.messages[1].chronology_key());
    assert!(!page.more);
    assert_eq!(page.start, 0);
}

#[test]
fn page_serves_boundaries_around_a_300_kib_record_by_skipping_it() {
    let scratch = Scratch::new("retained-history-300-kib-record");

    let page = page_around_oversized_record(&scratch, OVERSIZED_RECORD_BYTES);

    assert_both_boundaries_served(&page);
    assert_eq!(
        page.work.passes,
        3 * HISTORY_PASS_BUDGET,
        "the window ending at the record delimiter is skipped in one pass",
    );
    assert!(page.work.source_bytes > OVERSIZED_RECORD_BYTES as u64);
}

#[test]
fn page_serves_boundaries_around_a_record_wider_than_two_windows() {
    let scratch = Scratch::new("retained-history-600-kib-record");

    let page = page_around_oversized_record(&scratch, WIDE_RECORD_BYTES);

    assert_both_boundaries_served(&page);
    assert_eq!(
        page.work.passes,
        4 * HISTORY_PASS_BUDGET,
        "a window with no delimiter is skipped without a refusal",
    );
}

#[test]
fn resident_reload_does_not_manufacture_a_boundary_from_a_long_prose_tail() {
    let scratch = Scratch::new("prose-only-tail-bootstrap");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let contents = (0..300)
        .map(|index| assistant_record(index, &filler))
        .collect::<String>();
    fs::write(&transcript, &contents).expect("long prose transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    cursor.initialized = false;

    assert!(
        bootstrap_until_ready(&mut cursor, "prose-only tail bootstrap scans").is_empty(),
        "only typed durable compactions create boundaries",
    );
    assert!(cursor.initialized);
    assert_eq!(cursor.prefix_start, cursor.next);
}

#[test]
fn live_boundary_after_tail_bootstrap_authenticates_only_the_new_context_epoch() {
    const CALL: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"later\",\"name\":\"read\",\"arguments\":{\"path\":\"later.rs\"}}]}}\n";
    const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T19:44:10.271Z\"}\n";
    let scratch = Scratch::new("post-bootstrap-live-boundary");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let contents = (0..300)
        .map(|index| assistant_record(index, &filler))
        .collect::<String>();
    fs::write(&transcript, &contents).expect("long prose transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    cursor.initialized = false;
    assert!(bootstrap_until_ready(&mut cursor, "tail bootstrap scans").is_empty());
    let epoch_start = cursor.next;
    append(&transcript, format!("{CALL}{BOUNDARY}").as_bytes());

    reset_validation_bytes_read();
    let records = scan(&mut cursor).expect("live boundary scans");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].unavailable_tool_terminals, ["read: later.rs"]);
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
    let original = large_transcript('x');
    let replacement = large_transcript('y');
    assert_eq!(original.len(), replacement.len());
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

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
    const LATER_CALL: &str = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"later\",\"name\":\"read\",\"arguments\":{\"path\":\"later.rs\"}}]}}\n";
    const LATER_BOUNDARY: &str =
        "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:10.271Z\"}\n";
    let scratch = Scratch::new("append-during-validation");
    let transcript = scratch.transcript();
    fs::write(&transcript, large_transcript_with_pending('x', 192))
        .expect("large active transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

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
        ["read: held.rs"]
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
        ["read: held.rs"],
        "the appended call cannot enter the held boundary",
    );

    append(&transcript, LATER_BOUNDARY.as_bytes());
    let later = scan_until_boundary(&mut cursor, "appended records scan after validation");
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].unavailable_tool_terminals, ["read: later.rs"]);
    assert!(scan(&mut cursor).expect("settled poll scans").is_empty());
}

#[test]
fn same_length_mutation_after_admitted_growth_resets_validation() {
    const SUFFIX: &str = "{\"type\":\"progress\",\"serial\":1}\n";
    let scratch = Scratch::new("mutation-after-append");
    let transcript = scratch.transcript();
    fs::write(&transcript, large_transcript_with_pending('x', 192))
        .expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
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

    let replacement = format!("{}{SUFFIX}", large_transcript_with_pending('y', 192));
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
    let original = large_transcript_with_pending('x', 192);
    let mutation = u64::try_from(original.find("xxxx").expect("fixture text exists"))
        .expect("fixture offset fits u64");
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
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
    assert_eq!(records[0].unavailable_tool_terminals, ["read: held.rs"]);
}

#[test]
fn mutation_during_extension_authentication_discards_the_moving_slice() {
    const SUFFIX: &str = "{\"type\":\"progress\",\"serial\":1}\n";
    let scratch = Scratch::new("mutation-during-authentication");
    let transcript = scratch.transcript();
    let original = large_transcript_with_pending('x', 192);
    let mutation = u64::try_from(original.find("xxxx").expect("fixture text exists"))
        .expect("fixture offset fits u64");
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
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
    let original = large_transcript_with_pending('x', 192);
    let mutation = u64::try_from(original.find("xxxx").expect("fixture text exists"))
        .expect("fixture offset fits u64");
    fs::write(&transcript, original).expect("large original transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
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
    assert_eq!(records[0].unavailable_tool_terminals, ["read: held.rs"]);
}
