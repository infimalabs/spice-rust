use super::*;

#[test]
fn resident_restart_rebuilds_from_zero_with_fixed_work_and_no_partial_rows() {
    const BOUNDARY: &str = "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    let scratch = Scratch::new("history-auth-restart");
    let transcript = scratch.transcript();
    let filler = "x".repeat(1_800);
    let mut contents = (0..160)
        .map(|index| claude_assistant_record(index, &filler))
        .collect::<String>();
    let boundary_offset = contents.len() as u64;
    contents.push_str(BOUNDARY);
    fs::write(&transcript, contents).expect("large history generation is written");

    let abandoned = history_checkpoint(&transcript).expect("first resident captures source");
    let first = advance_history_authentication(&transcript, &abandoned, Instant::now())
        .expect("first resident advances one slice");
    assert_eq!(first.work.source_bytes, HISTORY_BYTE_BUDGET as u64);
    assert!(first.stamp.is_none());
    drop(abandoned);

    let restarted = history_checkpoint(&transcript).expect("replacement resident captures source");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    let restarted_first = bootstrap_before(&mut cursor, 1, &restarted, HistoryRead::Cursor)
        .expect("replacement resident advances one bounded slice");
    assert!(restarted_first.pending_authentication);
    assert!(restarted_first.records.is_empty());
    assert_eq!(
        restarted_first.work.source_bytes, HISTORY_BYTE_BUDGET as u64,
        "restart deliberately redoes its first slice rather than importing opaque hasher state"
    );

    let mut page = restarted_first;
    for _ in 0..4 {
        page = bootstrap_before(&mut cursor, 1, &restarted, HistoryRead::Cursor)
            .expect("replacement resident resumes bounded authentication");
        assert!(page.work.within_limits());
        if !page.pending_authentication {
            break;
        }
        assert!(
            page.records.is_empty(),
            "pending authority stays unpublished"
        );
    }
    assert!(!page.pending_authentication);
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].locus().offset, boundary_offset);
}

#[test]
fn rewrite_plus_growth_before_initial_build_finishes_is_refused() {
    let scratch = Scratch::new("history-auth-initial-rewrite");
    let transcript = scratch.transcript();
    fs::write(&transcript, vec![b'x'; HISTORY_BYTE_BUDGET + 17]).expect("large prefix is written");
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    assert_eq!(checkpoint.next(), 0);

    let first = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect("first authentication slice advances");
    assert!(first.stamp.is_none());
    assert_eq!(first.work.source_bytes, HISTORY_BYTE_BUDGET as u64);
    assert_eq!(first.work.passes, 1);
    assert!(first.work.within_limits());
    assert_eq!(checkpoint.next(), 0);
    let mut rewritten = OpenOptions::new()
        .write(true)
        .open(&transcript)
        .expect("prefix opens for rewrite");
    rewritten
        .seek(SeekFrom::Start(7))
        .expect("prefix rewrite seeks");
    rewritten.write_all(b"y").expect("prefix byte rewrites");
    drop(rewritten);
    append(&transcript, b"append");

    let error = (0..4)
        .find_map(|_| {
            advance_history_authentication(&transcript, &checkpoint, Instant::now()).err()
        })
        .expect("rewrite plus append must fail authentication");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn initial_authentication_advances_while_the_transcript_keeps_growing() {
    let scratch = Scratch::new("history-auth-growing");
    let transcript = scratch.transcript();
    let length = HISTORY_BYTE_BUDGET * 2 + 17;
    fs::write(&transcript, vec![b'x'; length]).expect("large prefix is written");
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");

    for _ in 0..6 {
        append(&transcript, b"append");
        let step = advance_history_authentication(&transcript, &checkpoint, Instant::now())
            .expect("append-only growth preserves authentication progress");
        assert!(step.stamp.is_none());
        assert!(step.work.within_limits());
    }

    let ready = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect("authenticated growing prefix becomes readable");
    assert!(ready.stamp.is_some());
    assert_eq!(checkpoint.next(), length as u64);
}

#[test]
fn genuine_append_reauthenticates_in_fixed_resumable_slices() {
    let scratch = Scratch::new("history-auth-append");
    let transcript = scratch.transcript();
    fs::write(&transcript, vec![b'x'; HISTORY_BYTE_BUDGET + 17]).expect("large prefix is written");
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    assert_eq!(checkpoint.next(), 0);
    let first = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect("first authentication slice advances");
    assert_eq!(first.work.source_bytes, HISTORY_BYTE_BUDGET as u64);
    assert_eq!(checkpoint.next(), 0);
    let resumed = checkpoint.clone();
    let initial_finish = advance_history_authentication(&transcript, &resumed, Instant::now())
        .expect("remaining initial prefix authenticates");
    assert!(initial_finish.stamp.is_none());
    assert_eq!(initial_finish.work.source_bytes, 17);
    let initial_ready = advance_history_authentication(&transcript, &resumed, Instant::now())
        .expect("initial authenticated horizon publishes");
    assert!(initial_ready.stamp.is_some());
    assert_eq!(resumed.next(), (HISTORY_BYTE_BUDGET + 17) as u64);
    append(&transcript, b"append");

    let recheck = advance_history_authentication(&transcript, &resumed, Instant::now())
        .expect("admitted prefix reauthenticates after append");
    assert!(recheck.stamp.is_none());
    assert_eq!(recheck.work.source_bytes, HISTORY_BYTE_BUDGET as u64);
    assert_eq!(resumed.next(), 0);
    let finish = advance_history_authentication(&transcript, &resumed, Instant::now())
        .expect("remaining original prefix authenticates");
    assert!(finish.stamp.is_none());
    assert_eq!(finish.work.source_bytes, 17);
    assert_eq!(resumed.next(), (HISTORY_BYTE_BUDGET + 17) as u64);
    let ready = advance_history_authentication(&transcript, &resumed, Instant::now())
        .expect("append-only generation becomes readable");
    assert!(ready.stamp.is_some());
    assert_eq!(ready.work, HistoryWork::default());
}

#[test]
fn visible_horizon_advances_only_after_every_slice_and_respects_retreat() {
    let scratch = Scratch::new("history-auth-visible-horizon");
    let transcript = scratch.transcript();
    let length = HISTORY_BYTE_BUDGET * 2 + 17;
    fs::write(&transcript, vec![b'x'; length]).expect("multi-slice prefix is written");
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    let retreated = checkpoint.retreat(29);

    for expected in [HISTORY_BYTE_BUDGET as u64, HISTORY_BYTE_BUDGET as u64] {
        let step = advance_history_authentication(&transcript, &checkpoint, Instant::now())
            .expect("authentication slice advances");
        assert_eq!(step.work.source_bytes, expected);
        assert!(step.stamp.is_none());
        assert_eq!(checkpoint.next(), 0);
        assert_eq!(retreated.next(), 0);
    }
    let finish = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect("final authentication slice advances");
    assert_eq!(finish.work.source_bytes, 17);
    assert!(finish.stamp.is_none());
    assert_eq!(checkpoint.next(), length as u64);
    assert_eq!(retreated.next(), 29);

    let ready = advance_history_authentication(&transcript, &retreated, Instant::now())
        .expect("authenticated retreated frontier publishes");
    assert!(ready.stamp.is_some());
    assert_eq!(ready.work, HistoryWork::default());
    assert_eq!(retreated.next(), 29);
}

#[test]
fn rows_are_discarded_when_source_moves_during_page_read() {
    const BOUNDARY: &str = "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n";
    let scratch = Scratch::new("history-auth-page-race");
    let transcript = scratch.transcript();
    fs::write(&transcript, BOUNDARY).expect("boundary is written");
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    assert!(
        advance_history_authentication(&transcript, &checkpoint, Instant::now())
            .expect("initial prefix hashes")
            .stamp
            .is_none()
    );
    let moving = transcript.clone();
    AFTER_HISTORY_SOURCE_READ.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut rewritten = OpenOptions::new()
                .write(true)
                .open(&moving)
                .expect("page prefix opens for rewrite");
            rewritten
                .seek(SeekFrom::Start(2))
                .expect("page rewrite seeks");
            rewritten.write_all(b"X").expect("page byte rewrites");
            drop(rewritten);
            append(&moving, b"{}\n");
        }));
    });
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);

    let page = bootstrap_before(&mut cursor, 1, &checkpoint, HistoryRead::Cursor)
        .expect("moving page is discarded before publication");

    assert!(page.pending_authentication);
    assert!(page.records.is_empty());
    let error = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect_err("rewritten page prefix must fail reauthentication");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("authenticated prefix was rewritten")
    );
}

#[test]
fn mutation_during_extension_recheck_discards_the_slice() {
    let scratch = Scratch::new("history-auth-inflight-rewrite");
    let transcript = scratch.transcript();
    fs::write(&transcript, vec![b'x'; 4 * 1024]).expect("prefix is written");
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    assert!(
        advance_history_authentication(&transcript, &checkpoint, Instant::now())
            .expect("initial prefix hashes")
            .stamp
            .is_none()
    );
    assert!(
        advance_history_authentication(&transcript, &checkpoint, Instant::now())
            .expect("initial prefix publishes")
            .stamp
            .is_some()
    );
    append(&transcript, b"first append");
    let moving = transcript.clone();
    AFTER_HISTORY_AUTHENTICATION_READ.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let mut rewritten = OpenOptions::new()
                .write(true)
                .open(&moving)
                .expect("prefix opens during authentication");
            rewritten
                .seek(SeekFrom::Start(9))
                .expect("in-flight rewrite seeks");
            rewritten
                .write_all(b"y")
                .expect("in-flight prefix byte rewrites");
            drop(rewritten);
            append(&moving, b"second append");
        }));
    });

    let moved = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect("moving authentication slice is discarded");
    assert!(moved.stamp.is_none());
    assert!(moved.work.within_limits());
    let error = advance_history_authentication(&transcript, &checkpoint, Instant::now())
        .expect_err("rewritten authenticated prefix must be rejected");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("authenticated prefix was rewritten")
    );
}
