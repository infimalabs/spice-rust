//! Reading acknowledgment outcomes out of recorded assistant turns.
//!
//! The fixtures are real-shaped turns kept beside this file and embedded at
//! compile time, so the scan these tests exercise performs no I/O and neither
//! does the test around it. Recognition therefore depends on the prose and the
//! standing it is read against, and on nothing else — the same property that
//! lets the same scan run at the command door and again during replay.

use std::collections::BTreeMap;

use steer::ack::keys::Key;
use steer::ack::scan::{Miss, Outcome, Recognition, Standing, scan};
use wire::Disposition;

const ACKNOWLEDGED: &str = include_str!("prose/acknowledged.md");
const REFUSED: &str = include_str!("prose/refused.md");
const MISSED: &str = include_str!("prose/missed.md");
const INDENTED_CODE: &str = include_str!("prose/indented-code.md");
const TILDE_FENCE: &str = include_str!("prose/tilde-fence.md");
const LONG_FENCE: &str = include_str!("prose/long-fence.md");

/// A header naming two live keys retires both, at the column each was written.
///
/// The turn also carries the other shapes a leading clause takes: indented
/// under a list, and separated from its keys by a tab. Neither changes what a
/// header means, so refusing either would drop a real acknowledgment over
/// whitespace.
#[test]
fn a_leading_clause_retires_every_key_it_names() {
    let board = board();

    assert_eq!(
        report(&scan(ACKNOWLEDGED, &board)),
        vec![
            "1:5 1kJK6N3R acked".to_owned(),
            "1:14 1kJK6M0V acked".to_owned(),
            "5:7 1kJK7Sqy acked".to_owned(),
            "7:5 1kJKB307 acked".to_owned(),
        ]
    );
}

/// One assistant message may distribute acknowledgements across separate
/// paragraphs. Each header remains an independent disposition rather than the
/// first header consuming the rest of the message as its body.
#[test]
fn one_distributed_message_extracts_every_acknowledgement_once() {
    let prose = concat!(
        "ACK 1kQwPZtK: The current completion path is singular and visible.\n\n",
        "ACK 1kQwPZtL: Historical TaskDone records are compatibility evidence only.",
    );

    assert_eq!(
        report(&scan(prose, &board())),
        vec![
            "1:5 1kQwPZtK acked".to_owned(),
            "3:5 1kQwPZtL acked".to_owned(),
        ]
    );
}

/// A refusal is a disposition, not an absence of one.
#[test]
fn a_refusal_retires_the_directive_as_declined() {
    let board = board();

    assert_eq!(
        report(&scan(REFUSED, &board)),
        vec!["1:6 1kJK7Sqy declined".to_owned()]
    );
}

/// Unknown, malformed, quoted, historical, and buried keys alter nothing and
/// are each reported by name. A silent non-recognition would be
/// indistinguishable from a refusal to an operator waiting on an answer.
#[test]
fn every_way_a_clause_fails_to_land_is_reported_by_name() {
    let board = board();

    assert_eq!(
        report(&scan(MISSED, &board)),
        vec![
            "3:5 1kJK6N3 malformed".to_owned(),
            "5:5 1kJKZZZZ unknown".to_owned(),
            "7:5 1kJK8KcW historical".to_owned(),
            "9:7 1kJK6N3R quoted".to_owned(),
            "11:21 1kJK6N3R buried".to_owned(),
            "14:5 1kJK6N3R quoted".to_owned(),
            "17:6 1kJK6N3R quoted".to_owned(),
            "20:6 1kJK6N3R quoted".to_owned(),
            "23:19 1kJK6M0V buried".to_owned(),
        ]
    );
}

/// Nothing in the missed turn touches the live keys it quotes.
#[test]
fn a_turn_that_only_quotes_keys_retires_none_of_them() {
    let board = board();

    for recognition in scan(MISSED, &board) {
        assert!(
            matches!(recognition.outcome, Outcome::Missed { .. }),
            "a quoted or misplaced clause retired something at {}",
            recognition.locus
        );
    }
}

#[test]
fn a_four_space_indented_code_clause_is_quoted() {
    assert_eq!(
        report(&scan(INDENTED_CODE, &board())),
        vec!["1:9 1kJK6N3R quoted".to_owned()],
    );
}

#[test]
fn a_tilde_fenced_clause_is_quoted() {
    assert_eq!(
        report(&scan(TILDE_FENCE, &board())),
        vec!["2:5 1kJK6M0V quoted".to_owned()],
    );
}

#[test]
fn a_shorter_inner_backtick_run_does_not_close_a_long_fence() {
    assert_eq!(
        report(&scan(LONG_FENCE, &board())),
        vec!["3:5 1kJK7Sqy quoted".to_owned()],
    );
}

/// Prose that uses the word without naming a key is not an attempt at all, so
/// ordinary sentences do not each produce a diagnostic.
#[test]
fn prose_about_acknowledging_is_silent() {
    let board = board();

    for quiet in [
        "I will ACK the following changes: the first and the second.",
        "The ACK protocol: a marker, a key, a colon.",
        "ACK",
        "ACK: nothing named",
        "acking is not ACKing 1kJK6N3R without a colon",
    ] {
        assert!(
            scan(quiet, &board).is_empty(),
            "{quiet:?} was read as an acknowledgment attempt"
        );
    }
}

/// A clause carrying one key-shaped token is diagnosed even when the token
/// beside it is not a key.
#[test]
fn a_leading_clause_judges_each_token_on_its_own() {
    let board = board();

    assert_eq!(
        report(&scan("ACK 1kJK6N3R nope: partly named.", &board)),
        vec![
            "1:5 1kJK6N3R acked".to_owned(),
            "1:14 nope malformed".to_owned(),
        ]
    );
}

/// One clause per line: a marker further along is prose about the first.
#[test]
fn only_the_first_clause_on_a_line_speaks() {
    let board = board();

    assert_eq!(
        report(&scan("ACK 1kJK6N3R: and then ACK 1kJK6M0V: again.", &board)),
        vec!["1:5 1kJK6N3R acked".to_owned()]
    );
}

/// A key from the retired base-62 mint retires where acknowledgments are
/// written, and goes unreported only where one was misplaced.
///
/// This is the entire cost of parsing being wider than recognition, stated as
/// behaviour rather than as a comment. A placed clause judges its tokens by the
/// grammar, which admits the old alphabet; a buried one is diagnosed only when a
/// token looks like something the current mint would write. The same sentence is
/// scanned twice with only the key changed, so what silences the first is the
/// alphabet and nothing else about the line.
#[test]
fn a_retired_alphabet_key_retires_when_placed_and_is_unreported_when_buried() {
    let retired = Board(
        [("YTVtQvOX", Disposition::Live)]
            .into_iter()
            .map(|(key, disposition)| {
                (
                    key.parse::<Key>().expect("retired-alphabet key"),
                    disposition,
                )
            })
            .collect(),
    );

    assert_eq!(
        report(&scan("ACK YTVtQvOX: heard.", &retired)),
        vec!["1:5 YTVtQvOX acked".to_owned()]
    );
    assert!(
        scan("I mentioned ACK YTVtQvOX: earlier.", &retired).is_empty(),
        "a buried pre-cutover key was diagnosed"
    );
    assert_eq!(
        report(&scan("I mentioned ACK 1kJK6N3R: earlier.", &board())),
        vec!["1:17 1kJK6N3R buried".to_owned()]
    );
}

/// Renders each recognition as `locus token outcome`, which is exactly the
/// diagnostic an operator needs to see why a key did or did not land.
fn report(recognitions: &[Recognition]) -> Vec<String> {
    recognitions
        .iter()
        .map(|recognition| match &recognition.outcome {
            Outcome::Retired { key, disposition } => {
                let word = steer::ack::keys::DISPOSITIONS
                    .iter()
                    .find(|(_, admitted)| admitted == disposition)
                    .map_or("live", |(word, _)| word);
                format!("{} {key} {word}", recognition.locus)
            }
            Outcome::Missed { token, miss } => {
                format!("{} {token} {}", recognition.locus, miss.as_str())
            }
        })
        .collect()
}

/// The standing the fixtures are read against.
struct Board(BTreeMap<Key, Disposition>);

impl Standing for Board {
    fn disposition(&self, key: &Key) -> Option<Disposition> {
        self.0.get(key).copied()
    }
}

fn board() -> Board {
    Board(
        [
            ("1kJK6N3R", Disposition::Live),
            ("1kJK6M0V", Disposition::Live),
            ("1kJK7Sqy", Disposition::Live),
            ("1kJKB307", Disposition::Live),
            ("1kJK8KcW", Disposition::Acked),
            ("1kQwPZtK", Disposition::Live),
            ("1kQwPZtL", Disposition::Live),
        ]
        .into_iter()
        .map(|(key, disposition)| (key.parse::<Key>().expect("fixture key"), disposition))
        .collect(),
    )
}

/// The scanner reads the same outcomes out of prose whichever ledger reduced
/// the facts, so the ledger itself answers as standing.
#[test]
fn the_ledger_answers_as_standing() {
    use steer::directives::{Ledger, Steering};
    use wire::body::SteerSent;

    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Sent(&SteerSent {
            directive: "00000000-0000-4000-8000-000000000001"
                .parse()
                .expect("fixture directive UUID"),
            fanout: None,
            key: "1kJK6N3R".to_owned(),
            lane: "018f82d4-6f59-7d06-92e3-bce913116302"
                .parse()
                .expect("lane"),
            text: "hold the line".to_owned(),
            control: None,
            attachments: Vec::new(),
            expires: "2026-08-01T07:07:19.000Z".parse().expect("instant"),
        }))
        .expect("sent");

    assert_eq!(
        report(&scan("ACK 1kJK6N3R: heard.", &ledger)),
        vec!["1:5 1kJK6N3R acked".to_owned()]
    );
    assert_eq!(
        report(&scan("ACK 1kJK6M0V: never sent.", &ledger)),
        vec!["1:5 1kJK6M0V unknown".to_owned()]
    );
}

/// Miss words are the ones the diagnostics are written in.
#[test]
fn every_miss_is_reported_under_one_word() {
    for (miss, word) in [
        (Miss::Buried, "buried"),
        (Miss::Quoted, "quoted"),
        (Miss::Malformed, "malformed"),
        (Miss::Unknown, "unknown"),
        (Miss::Historical, "historical"),
    ] {
        assert_eq!(miss.as_str(), word);
        assert_eq!(miss.to_string(), word);
    }
}
