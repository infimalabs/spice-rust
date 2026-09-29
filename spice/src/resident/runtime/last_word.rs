//! The word an exiting occupant's transcript ends on.
//!
//! An occupant whose turn the vendor refused writes that refusal as its last
//! event and exits cleanly, so its exit status says nothing and the notice for
//! the refusal may still be unread when the exit is accounted. The tape is
//! durable before either notice is sent, so the exit reads the tape itself:
//! the newest event the occupant wrote, scanned back from the tail through the
//! session declaration that opened it, decides whether the exit ended on the
//! vendor's word, on the occupant's own answer, or on nothing at all.

use super::*;

/// Most tail bytes read back looking for the occupant's newest event.
///
/// The final records of a refused turn are small; a tail this long is read
/// only when they sit behind large tool results.
const TAIL_BYTES: u64 = 1024 * 1024;

/// What the newest event an exiting occupant wrote says about its exit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum LastWord {
    /// The vendor refused the turn, in these words.
    Failure(String),
    /// The vendor answered: the occupant's newest event is assistant prose,
    /// reasoning, a tool exchange, reported usage, or a compaction.
    Answered,
    /// Nothing decides the exit: the occupant wrote no event past its
    /// declaration, its newest word is the operator's own prompt, or its
    /// transcript could not be read.
    Silent,
}

/// The word the exiting occupant's transcript ends on.
///
/// Only records at or after the occupant's own session declaration are read,
/// so a failure an earlier occupant left on the tape never holds the lane
/// against a successor that wrote nothing. An occupant that declared no
/// session has no bounded range and is left to ordinary death accounting.
/// Every reading failure is written down and answers [`LastWord::Silent`]:
/// the exit is accounted either way, and this decides only how.
pub(super) fn exit_word(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<LastWord> {
    let observed = current_residency(bus, epoch)?;
    let Some(session) = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == lane && row.occupant == Some(occupant))
        .and_then(|row| row.session.as_ref())
        .filter(|session| session.occupant == occupant)
    else {
        return Ok(LastWord::Silent);
    };
    let Some(vendor) = ingest_vendor(&session.vendor) else {
        return Ok(LastWord::Silent);
    };
    let root = paths
        .tape
        .join(lane.to_string())
        .join(session.vendor.as_str());
    match closing_word(&root, vendor, &session.locus) {
        Ok(word) => Ok(word),
        Err(error) => {
            let _ = writeln!(
                diagnostic,
                "lane {lane} occupant {occupant} closing transcript unread; exit accounted without it: {error}"
            );
            Ok(LastWord::Silent)
        }
    }
}

/// The word the newest event at or after `declared` is, read from the tape
/// under `root` with the vendor's dialect.
pub(super) fn closing_word(
    root: &Path,
    vendor: &spice::extension::Vendor,
    declared: &wire::TapeLocus,
) -> Result<LastWord, String> {
    let tape = tape::Tape::open_existing(root).map_err(|error| error.to_string())?;
    let extents = tape.extents().map_err(|error| error.to_string())?;
    let mut remaining = TAIL_BYTES;
    for extent in extents.into_iter().rev() {
        if extent.member < declared.member || remaining == 0 {
            break;
        }
        let start = if extent.member == declared.member {
            declared.offset
        } else {
            0
        };
        let end = extent.length;
        let length = end.saturating_sub(start).min(remaining);
        if length == 0 {
            continue;
        }
        let offset = end - length;
        let bytes = tape
            .read(tape::Locus::new(extent.member, offset, length))
            .map_err(|error| error.to_string())?;
        remaining -= length;
        // A window cut short of the declaration may open mid-record; only a
        // range that follows a newline, or starts at the declaration, is whole.
        let whole_from_start = offset == start;
        for (from, to) in record_ranges(&bytes, whole_from_start).into_iter().rev() {
            let locus = tape::Locus::new(
                extent.member,
                offset + u64::try_from(from).map_err(|error| error.to_string())?,
                u64::try_from(to - from).map_err(|error| error.to_string())?,
            );
            let decoded = vendor
                .decode(&bytes[from..to], locus)
                .map_err(|error| error.to_string())?;
            // A sign-out is the vendor's account notice, not a word on the
            // turn, so the exit is decided by what the occupant wrote before it.
            let Some(newest) = decoded
                .events()
                .iter()
                .rev()
                .find(|event| !matches!(event, listen::Event::SignOut(_)))
            else {
                continue;
            };
            return Ok(match newest {
                listen::Event::Failure(failure) => LastWord::Failure(failure.message.clone()),
                listen::Event::Prose(prose) if prose.role == listen::ProseRole::Operator => {
                    LastWord::Silent
                }
                listen::Event::Session(_) | listen::Event::SignOut(_) => LastWord::Silent,
                listen::Event::Prose(_)
                | listen::Event::ProseEvidence(_)
                | listen::Event::Reasoning(_)
                | listen::Event::Tool(_)
                | listen::Event::Usage(_)
                | listen::Event::Compaction(_) => LastWord::Answered,
            });
        }
    }
    Ok(LastWord::Silent)
}

/// Complete newline-terminated record ranges in `bytes`, skipping a leading
/// partial record unless the window is known to open on a record boundary.
fn record_ranges(bytes: &[u8], whole_from_start: bool) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = if whole_from_start { Some(0) } else { None };
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if let Some(from) = start {
            ranges.push((from, index + 1));
        }
        start = Some(index + 1);
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const REFUSAL: &str =
        "Refusal (reasoning_extraction): This request was blocked by our content filtering policy.";

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-last-word-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).expect("last-word scratch root is unique");
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("last-word scratch is removed");
        }
    }

    fn write(tape: &tape::Tape, record: &str) -> wire::TapeLocus {
        let locus = tape
            .tee(format!("{record}\n").as_bytes())
            .expect("fixture record appends");
        wire::TapeLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        }
    }

    fn omp_declaration(session: &str) -> String {
        format!(
            r#"{{"id":"spice-state","type":"response","command":"get_state","success":true,"data":{{"sessionId":"{session}"}}}}"#
        )
    }

    fn omp_assistant(stop: &str, text: &str, error: Option<&str>) -> String {
        let error = error.map_or(String::new(), |error| {
            format!(r#","errorMessage":"{error}""#)
        });
        format!(
            r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}],"usage":{{"input":40,"output":0}},"stopReason":"{stop}"{error}}}}}"#
        )
    }

    const OMP_USER: &str = r#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"continue"}]}}"#;
    const OMP_TURN_END: &str =
        r#"{"type":"turn_end","message":{"role":"assistant","content":[],"stopReason":"error"}}"#;
    const OMP_WIDGET: &str =
        r#"{"type":"extension_ui_request","id":"1583f94d24ebac0b","method":"setWidget"}"#;
    const OMP_AGENT_END: &str = r#"{"type":"agent_end","messages":[]}"#;

    fn refused_wake(tape: &tape::Tape, session: &str) -> wire::TapeLocus {
        write(tape, r#"{"type":"ready","protocolVersion":1}"#);
        let declared = write(tape, &omp_declaration(session));
        write(tape, r#"{"type":"agent_start"}"#);
        write(tape, r#"{"type":"turn_start"}"#);
        write(tape, OMP_USER);
        write(tape, &omp_assistant("error", "", Some(REFUSAL)));
        write(tape, OMP_TURN_END);
        write(tape, OMP_WIDGET);
        write(tape, OMP_AGENT_END);
        declared
    }

    #[test]
    fn a_wake_whose_only_turn_the_vendor_refused_ends_on_the_vendor_error() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        let declared = refused_wake(&tape, "018f82d4-6f59-7d06-92e3-bce9131163c2");

        let word = closing_word(&scratch.0, &spice::extension::OMP, &declared)
            .expect("the retained tape reads");
        assert_eq!(word, LastWord::Failure(REFUSAL.to_owned()));
    }

    #[test]
    fn a_turn_that_ended_on_prose_is_an_answer() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        let declared = write(
            &tape,
            &omp_declaration("018f82d4-6f59-7d06-92e3-bce9131163c2"),
        );
        write(&tape, OMP_USER);
        write(&tape, &omp_assistant("stop", "done", None));
        write(&tape, OMP_TURN_END);
        write(&tape, OMP_AGENT_END);

        let word = closing_word(&scratch.0, &spice::extension::OMP, &declared)
            .expect("the retained tape reads");
        assert_eq!(word, LastWord::Answered);
    }

    #[test]
    fn a_turn_that_ended_on_the_operators_prompt_is_silent() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        let declared = write(
            &tape,
            &omp_declaration("018f82d4-6f59-7d06-92e3-bce9131163c2"),
        );
        write(&tape, r#"{"type":"agent_start"}"#);
        write(&tape, OMP_USER);
        write(&tape, OMP_WIDGET);

        let word = closing_word(&scratch.0, &spice::extension::OMP, &declared)
            .expect("the retained tape reads");
        assert_eq!(word, LastWord::Silent);
    }

    #[test]
    fn a_predecessor_failure_before_the_declaration_never_holds_the_successor() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        let predecessor = refused_wake(&tape, "018f82d4-6f59-7d06-92e3-bce9131163c2");
        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &predecessor)
                .expect("the retained tape reads"),
            LastWord::Failure(REFUSAL.to_owned()),
            "the predecessor's exit is accounted on its refusal"
        );

        let successor = write(
            &tape,
            &omp_declaration("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        );
        write(&tape, r#"{"type":"agent_start"}"#);
        write(&tape, OMP_AGENT_END);
        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &successor)
                .expect("the retained tape reads"),
            LastWord::Silent,
            "the successor wrote no event after its declaration"
        );
        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &predecessor)
                .expect("the retained tape reads"),
            LastWord::Silent,
            "the successor's declaration is now the newest word on the tape"
        );
        let beyond = wire::TapeLocus {
            member: successor.member,
            offset: successor.offset + 4096,
            length: 1,
        };
        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &beyond)
                .expect("a declaration past the tape reads nothing"),
            LastWord::Silent
        );
    }

    #[test]
    fn the_scan_crosses_sealed_members_back_to_the_declaration() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        write(
            &tape,
            &omp_assistant("error", "", Some("an earlier occupant's refusal")),
        );
        let declared = write(
            &tape,
            &omp_declaration("018f82d4-6f59-7d06-92e3-bce9131163c2"),
        );
        tape.seal().expect("first member seals");
        write(&tape, OMP_USER);
        tape.seal().expect("second member seals");
        write(&tape, &omp_assistant("error", "", Some(REFUSAL)));
        write(&tape, OMP_TURN_END);
        write(&tape, OMP_AGENT_END);
        assert_eq!(declared.member, 1);

        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &declared)
                .expect("the sealed tape reads"),
            LastWord::Failure(REFUSAL.to_owned())
        );

        let later = wire::TapeLocus {
            member: 3,
            offset: 0,
            length: 1,
        };
        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &later)
                .expect("the sealed tape reads"),
            LastWord::Failure(REFUSAL.to_owned())
        );
        let last_member_only = wire::TapeLocus {
            member: 2,
            offset: 0,
            length: 1,
        };
        assert_eq!(
            closing_word(&scratch.0, &spice::extension::OMP, &last_member_only)
                .expect("the sealed tape reads"),
            LastWord::Failure(REFUSAL.to_owned())
        );
    }

    #[test]
    fn a_missing_tape_is_a_reading_failure_not_a_verdict() {
        let scratch = Scratch::new();
        let declared = wire::TapeLocus {
            member: 1,
            offset: 0,
            length: 1,
        };
        closing_word(&scratch.0.join("absent"), &spice::extension::OMP, &declared)
            .expect_err("an absent tape cannot decide the exit");
    }

    #[test]
    fn record_ranges_skip_a_leading_partial_unless_the_window_opens_on_a_boundary() {
        let bytes = b"tail-of-record\nwhole\nlast\npartial";
        assert_eq!(record_ranges(bytes, false), [(15, 21), (21, 26)]);
        assert_eq!(record_ranges(bytes, true), [(0, 15), (15, 21), (21, 26)]);
        assert!(record_ranges(b"no newline", false).is_empty());
        assert!(record_ranges(b"", true).is_empty());
    }
}
