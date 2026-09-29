use super::*;

const BOUNDARY: &str = "{\"type\":\"compaction\",\"timestamp\":\"2030-04-12T18:44:09.271Z\"}\n";

/// Write a transcript several history windows long, one boundary behind every
/// run of assistant records, and return each boundary's offset.
fn windowed_transcript(transcript: &Path) -> Vec<u64> {
    let filler = "x".repeat(1_800);
    let mut contents = String::new();
    let mut boundaries = Vec::new();
    for boundary in 0..30 {
        contents.extend((0..24).map(|index| assistant_record(boundary * 24 + index, &filler)));
        boundaries.push(contents.len() as u64);
        contents.push_str(BOUNDARY);
    }
    fs::write(transcript, contents).expect("windowed transcript is written");
    boundaries
}

fn rewrite_byte(path: &Path, offset: u64) {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("transcript opens for rewrite");
    file.seek(SeekFrom::Start(offset))
        .expect("prefix rewrite seeks");
    file.write_all(b"\0").expect("prefix byte rewrites");
}

fn after_authentication_read(hook: impl FnOnce() + 'static) {
    AFTER_HISTORY_AUTHENTICATION_READ.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[test]
fn history_pages_a_transcript_appended_before_every_window() {
    let scratch = Scratch::new("history-auth-live-append");
    let transcript = scratch.transcript();
    let boundaries = windowed_transcript(&transcript);
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    let mut end = checkpoint.clone();
    let mut served = Vec::new();

    for _ in 0..64 {
        append(&transcript, b"{\"type\":\"progress\"}\n");
        let page = bootstrap_before(&mut cursor, 0, &end, HistoryRead::Page)
            .expect("a growing transcript serves its window");
        assert!(page.work.within_limits());
        served.splice(
            0..0,
            page.records.iter().map(|record| record.locus().offset),
        );
        if page.start == 0 {
            break;
        }
        end = checkpoint.retreat(page.start);
    }

    assert_eq!(
        served, boundaries,
        "every window of the growing transcript is served once, in order",
    );
}

#[test]
fn rewrite_plus_append_of_read_prefix_bytes_is_refused() {
    let scratch = Scratch::new("history-auth-rewrite");
    let transcript = scratch.transcript();
    let boundaries = windowed_transcript(&transcript);
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);
    bootstrap_before(&mut cursor, 0, &checkpoint, HistoryRead::Page).expect("newest window reads");

    rewrite_byte(&transcript, checkpoint.next() - 200);
    append(&transcript, b"{}\n");
    let error = refused(&mut cursor, &checkpoint, "a rewritten window is refused");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("authenticated prefix was rewritten")
    );
    let untouched = checkpoint.retreat(boundaries[5]);
    let error = refused(&mut cursor, &untouched, "a rewritten source stays refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn a_write_during_a_first_read_serves_appends_and_refuses_rewrites() {
    let scratch = Scratch::new("history-auth-concurrent-write");
    let transcript = scratch.transcript();
    let boundaries = windowed_transcript(&transcript);
    let checkpoint = history_checkpoint(&transcript).expect("history checkpoint starts");
    let mut cursor = cursor(&transcript, listen::Dialect::Omp);

    let appending = transcript.clone();
    after_authentication_read(move || append(&appending, b"{}\n"));
    let newest = bootstrap_before(&mut cursor, 0, &checkpoint, HistoryRead::Page)
        .expect("an append during the read serves the window");
    assert_eq!(
        newest.records.last().map(|record| record.locus().offset),
        boundaries.last().copied(),
    );

    let rewriting = transcript.clone();
    let target = newest.start - HISTORY_BYTE_BUDGET as u64 + 1;
    after_authentication_read(move || {
        rewrite_byte(&rewriting, target);
        append(&rewriting, b"{}\n");
    });
    let error = refused(
        &mut cursor,
        &checkpoint.retreat(newest.start),
        "a rewrite during the first read is refused",
    );
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("while its prefix was being read")
    );
}

fn refused(cursor: &mut Cursor, end: &HistoryCheckpoint, context: &str) -> io::Error {
    let Err(error) = bootstrap_before(cursor, 0, end, HistoryRead::Page) else {
        panic!("{context}");
    };
    error
}
