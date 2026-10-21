//! Reading acknowledgment outcomes out of recorded assistant turns.
//!
//! The fixtures are real-shaped turns kept beside this file and embedded at
//! compile time, so the scan these tests exercise performs no I/O and neither
//! does the test around it. Recognition therefore depends on the prose and the
//! standing it is read against, and on nothing else — the same property that
//! lets the same scan run at the command door and again during replay.

use std::collections::BTreeMap;

use steer::ack::keys::Key;
use steer::ack::scan::{
    Miss, Outcome, Recognition, Standing, decisive, has_acknowledgement, scan,
    scan_acknowledgements,
};
use wire::Disposition;

const ACKNOWLEDGED: &str = include_str!("prose/acknowledged.md");
const REFUSED: &str = include_str!("prose/refused.md");
const MISSED: &str = include_str!("prose/missed.md");
const INDENTED_CODE: &str = include_str!("prose/indented-code.md");
const TILDE_FENCE: &str = include_str!("prose/tilde-fence.md");
const LONG_FENCE: &str = include_str!("prose/long-fence.md");

/// A header naming two live keys retires both, at the column each was written.
///
/// The turn also carries indented prose and a tab separating a marker from its
/// key. Neither changes what a header means, so refusing either would drop a
/// real acknowledgment over whitespace.
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

/// Standing is loaded only when the canonical grammar sees an authored
/// attempt; preflight and scanning must share multiline and context guards.
#[test]
fn acknowledgement_preflight_uses_the_canonical_clause_grammar() {
    for prose in [
        "ACK 1kJK6N3R: spaced.",
        "ACK\t1kJK6N3R: tabbed.",
        "NAK\t1kJK7Sqy: declined.",
        "Progress follows: ACK 1kJK6N3R. applied.",
        "ACK\n1kJK6N3R\n: cross-line.",
        "**ACK** **1kJK6N3R**:** emphasized.",
    ] {
        assert!(has_acknowledgement(prose), "missed {prose:?}");
    }

    for prose in [
        "> ACK\t1kJK6N3R: quoted.",
        "    NAK\t1kJK7Sqy: code.",
        "I do not ACK 1kJK6N3R: negated.",
        "If I ACK 1kJK6N3R: hypothetical.",
        "The example ACK 1kJK6N3R: narrated.",
        "I emitted ACK 1kJK6N3R: narration about prior prose.",
        "The parser recognizes ACK 1kJK6N3R: as protocol.",
        "This sentence contains ACK 1kJK6N3R: as an example.",
        "src/ack.rs:12: ACK 1kJK6N3R: rendered source context.",
        "- ACK 1kJK6N3R: list example.",
        "1. NAK 1kJK7Sqy: list example.",
    ] {
        assert!(!has_acknowledgement(prose), "accepted {prose:?}");
    }
}

/// Separator punctuation and emphasis are presentation around one key grammar.
#[test]
fn current_header_forms_share_one_packet_grammar() {
    for (prose, expected, separator) in [
        ("ACK 1kJK6N3R: applied.", "1:5 1kJK6N3R acked", Some(':')),
        ("ACK 1kJK6N3R. applied.", "1:5 1kJK6N3R acked", Some('.')),
        ("ACK 1kJK6N3R applied.", "1:5 1kJK6N3R acked", None),
        (
            "**ACK 1kJK6N3R:** applied.",
            "1:7 1kJK6N3R acked",
            Some(':'),
        ),
        (
            "**ACK** **1kJK6N3R**:** applied.",
            "1:11 1kJK6N3R acked",
            Some(':'),
        ),
    ] {
        let packets = scan_acknowledgements(prose, &board());
        assert_eq!(packets.len(), 1, "missed {prose:?}");
        assert_eq!(
            packets[0].separator, separator,
            "wrong separator for {prose:?}"
        );
        assert_eq!(report(&packets[0].recognitions), [expected]);
        assert!(!prose[packets[0].start..packets[0].end].ends_with(' '));
    }
}

/// Marker, key list, and separator line breaks remain part of one packet. The
/// packet ranges are what presentation uses to attribute each multiline body.
#[test]
fn multiline_headers_preserve_exact_loci_and_body_boundaries() {
    let prose = concat!(
        "Before.\nACK\n1kJK6N3R\n1kJK6M0V\n:\nApplied together.\n",
        "NAK\n1kJK7Sqy\n\u{2014}\nUnsafe.",
    );
    let packets = scan_acknowledgements(prose, &board());

    assert_eq!(packets.len(), 2);
    assert_eq!(
        &prose[packets[0].start..packets[0].end],
        "ACK\n1kJK6N3R\n1kJK6M0V\n:"
    );
    assert_eq!(
        report(&packets[0].recognitions),
        ["3:1 1kJK6N3R acked", "4:1 1kJK6M0V acked"]
    );
    assert_eq!(
        &prose[packets[0].end..packets[1].start],
        "\nApplied together.\n"
    );
    assert_eq!(
        &prose[packets[1].start..packets[1].end],
        "NAK\n1kJK7Sqy\n\u{2014}"
    );
    assert_eq!(report(&packets[1].recognitions), ["8:1 1kJK7Sqy declined"]);
    assert_eq!(&prose[packets[1].end..], "\nUnsafe.");
}

/// Every prose boundary ends the key list for both dispositions. The Unicode
/// dashes are multi-byte separators, while a bare space remains an unconsumed
/// boundary rather than punctuation owned by the packet.
#[test]
fn ack_and_nak_key_lists_stop_at_every_prose_delimiter() {
    for (delimiter, separator) in [
        (":", Some(':')),
        (".", Some('.')),
        (";", Some(';')),
        (",", Some(',')),
        ("!", Some('!')),
        ("?", Some('?')),
        ("\u{2013}", Some('\u{2013}')),
        ("\u{2014}", Some('\u{2014}')),
        (" ", None),
    ] {
        for (marker, key, column, disposition) in [
            ("ACK", "1kJK6N3R", 5, "acked"),
            ("NAK", "1kJK7Sqy", 5, "declined"),
        ] {
            let prose = format!("{marker} {key}{delimiter}ordinary response prose");
            let packets = scan_acknowledgements(&prose, &board());

            assert_eq!(packets.len(), 1, "missed {prose:?}");
            assert_eq!(
                packets[0].separator, separator,
                "wrong separator for {prose:?}"
            );
            assert_eq!(
                report(&packets[0].recognitions),
                [format!("1:{column} {key} {disposition}")]
            );
            assert!(
                packets[0].end <= prose.find("ordinary").expect("fixture has response prose"),
                "response prose was consumed by {prose:?}"
            );
        }
    }
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

/// Keys listed with commas are the same header as keys listed with spaces;
/// a comma that nothing keyed follows is the header's separator, as before.
#[test]
fn a_comma_between_keys_lists_them_and_a_comma_after_the_last_key_separates() {
    let board = board();

    assert_eq!(
        report(&scan("ACK 1kJK6N3R, 1kJK6M0V: both handled.", &board)),
        vec![
            "1:5 1kJK6N3R acked".to_owned(),
            "1:15 1kJK6M0V acked".to_owned(),
        ],
    );
    let packets = scan_acknowledgements("ACK 1kJK6N3R, then the rest.", &board);
    assert_eq!(packets.len(), 1);
    assert_eq!(report(&packets[0].recognitions), ["1:5 1kJK6N3R acked"]);
    assert_eq!(packets[0].separator, Some(','));
}

/// Historical standing changes disposition, not packet membership: a later
/// key in the same header must still be parsed and judged independently.
#[test]
fn a_mixed_historical_and_live_header_parses_every_key_in_source_order() {
    let standing = Board(
        [
            ("1kR5x8yR", Disposition::Acked),
            ("1kR5xJDD", Disposition::Live),
        ]
        .into_iter()
        .map(|(key, disposition)| (key.parse::<Key>().expect("fixture key"), disposition))
        .collect(),
    );

    assert_eq!(
        report(&scan(
            "ACK 1kR5x8yR 1kR5xJDD: One response covers both keys.",
            &standing,
        )),
        vec![
            "1:5 1kR5x8yR historical".to_owned(),
            "1:14 1kR5xJDD acked".to_owned(),
        ],
    );
}

#[test]
fn one_header_retires_mixed_width_keys_at_longest_valid_boundaries() {
    let retained = "1kR5xJDD".parse::<Key>().expect("retained key");
    let pre_codec = "YTVtQvOX".parse::<Key>().expect("pre-codec retained key");
    let current = Key::for_micros(1_788_810_000_000_000).expect("current key");
    let standing = Board(BTreeMap::from([
        (retained.clone(), Disposition::Live),
        (pre_codec.clone(), Disposition::Live),
        (current.clone(), Disposition::Live),
    ]));
    let prose = format!("ACK {retained} {pre_codec} {current}: every retained mint retires.");
    let outcomes = scan(&prose, &standing);
    assert_eq!(outcomes.len(), 3);
    assert!(matches!(
        &outcomes[0].outcome,
        Outcome::Retired { key, .. } if key == &retained
    ));
    assert!(matches!(
        &outcomes[1].outcome,
        Outcome::Retired { key, .. } if key == &pre_codec
    ));
    assert!(matches!(
        &outcomes[2].outcome,
        Outcome::Retired { key, .. } if key == &current
    ));

    for malformed in [
        format!("ACK {current}0: overlong."),
        "ACK 1kR5xJDDa: legacy token followed by a non-current digit.".to_owned(),
        "ACK 1kR5xADDo: current width with a vowel.".to_owned(),
    ] {
        assert!(
            scan(&malformed, &standing).is_empty(),
            "accepted {malformed:?}"
        );
    }
}

/// A refusal is a disposition, not an absence of one.
#[test]
fn a_refusal_retires_the_directive_as_declined() {
    let board = board();

    assert_eq!(
        report(&scan(REFUSED, &board)),
        vec!["1:5 1kJK7Sqy declined".to_owned()]
    );
}

/// Unknown, quoted, and historical keys alter nothing and are reported by
/// name. Guarded narration is ignored; a malformed non-key stays silent.
#[test]
fn every_way_a_clause_fails_to_land_is_reported_by_name() {
    let board = board();

    assert_eq!(
        report(&scan(MISSED, &board)),
        vec![
            "5:5 1kJKZZZZ unknown".to_owned(),
            "7:5 1kJK8KcW historical".to_owned(),
            "9:7 1kJK6N3R quoted".to_owned(),
            "14:5 1kJK6N3R quoted".to_owned(),
            "17:6 1kJK6N3R quoted".to_owned(),
            "20:5 1kJK6N3R quoted".to_owned(),
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
        "I will ACK something after checking the result.",
    ] {
        assert!(
            scan(quiet, &board).is_empty(),
            "{quiet:?} was read as an acknowledgment attempt"
        );
    }
}

/// A key run ends when response prose begins, regardless of separator choice.
#[test]
fn a_leading_clause_stops_before_non_key_response_prose() {
    let board = board();

    assert_eq!(
        report(&scan("ACK 1kJK6N3R nope: partly named.", &board)),
        vec!["1:5 1kJK6N3R acked".to_owned()]
    );
}

/// Every unguarded marker starts another authentic packet, even when ordinary
/// response prose precedes it on the same line.
#[test]
fn an_unseparated_second_clause_on_a_line_also_speaks() {
    let board = board();

    assert_eq!(
        report(&scan("ACK 1kJK6N3R: and then ACK 1kJK6M0V: again.", &board)),
        vec![
            "1:5 1kJK6N3R acked".to_owned(),
            "1:28 1kJK6M0V acked".to_owned(),
        ]
    );
}

/// Sentence punctuation starts another authentic packet on the same line.
#[test]
fn sentence_delimited_packets_on_one_line_each_speak() {
    let board = board();

    assert_eq!(
        report(&scan(
            "ACK 1kJK6N3R: first handled. ACK 1kJK6M0V: second handled.",
            &board,
        )),
        vec![
            "1:5 1kJK6N3R acked".to_owned(),
            "1:34 1kJK6M0V acked".to_owned(),
        ]
    );
}

/// Retained-alphabet keys remain valid wherever authored ACK prose appears;
/// explicit narration about such a header remains silent.
#[test]
fn a_retired_alphabet_key_retires_anywhere_in_authored_prose() {
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
        report(&scan("Handled inline: ACK YTVtQvOX: heard.", &retired)),
        vec!["1:21 YTVtQvOX acked".to_owned()]
    );
    assert!(
        scan("I mentioned ACK YTVtQvOX: earlier.", &retired).is_empty(),
        "narrated protocol prose must not retire the key"
    );
}

/// The header an operator reads decides the key, however the prose goes on
/// to talk about it.
///
/// A lane that declines and then explains itself by quoting the header it
/// wrote earlier has still declined. The scan reports quoted mentions by name,
/// so diagnostics retain context while the authored header remains decisive.
#[test]
fn a_placed_header_decides_its_key_over_a_later_quoted_mention() {
    let board = board();
    let prose = concat!(
        "NAK 1kJK7Sqy: Verified on the tape; the direction names another lane.\n\n",
        "I emitted `ACK 1kJK7Sqy: Carrying it forward now` as prose before checking.\n",
        "`ACK 1kJK7Sqy: Carrying it forward now` is the header the operator saw.",
    );

    assert_eq!(
        report(&scan(prose, &board)),
        vec![
            "1:5 1kJK7Sqy declined".to_owned(),
            "3:16 1kJK7Sqy quoted".to_owned(),
            "4:6 1kJK7Sqy quoted".to_owned(),
        ]
    );
    assert_eq!(
        decided(prose, "1kJK7Sqy", &board),
        Some("1:5 1kJK7Sqy declined".to_owned())
    );
}

/// Authored clauses outrank quoted mentions, and among authored clauses the
/// later one speaks.
#[test]
fn the_decisive_recognition_is_the_last_of_the_highest_rank() {
    let board = board();

    for (prose, expected) in [
        (
            "Earlier I wrote ACK 1kJK6N3R: no.\nACK 1kJK6N3R: yes.",
            Some("2:5 1kJK6N3R acked"),
        ),
        (
            "ACK 1kJK6N3R: first thought.\nNAK 1kJK6N3R: on reflection, no.",
            Some("2:5 1kJK6N3R declined"),
        ),
        (
            "> ACK 1kJK6N3R: quoted\nI said ACK 1kJK6N3R: narrated",
            Some("1:7 1kJK6N3R quoted"),
        ),
        ("ACK 1kJK6M0V: another key entirely.", None),
    ] {
        assert_eq!(
            decided(prose, "1kJK6N3R", &board),
            expected.map(str::to_owned),
            "{prose:?}"
        );
    }
    assert_eq!(
        decided(
            "ACK 1kJK8KcW: already retired.\n`ACK 1kJK8KcW: shown`",
            "1kJK8KcW",
            &board
        ),
        Some("1:5 1kJK8KcW historical".to_owned()),
        "a miss judged on the key itself is still placed"
    );
}

/// Renders the decisive recognition for one token the way [`report`] does.
fn decided(prose: &str, token: &str, standing: &impl Standing) -> Option<String> {
    decisive(prose, token, standing).map(|recognition| report(&[recognition]).remove(0))
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
            from: None,
            room: None,
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
