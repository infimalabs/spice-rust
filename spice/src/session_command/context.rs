use super::*;
use spice::repository::Repository;

const WINDOW_BYTES: u64 = 256 * 1024;
const WINDOW_RECORDS: usize = 8;

pub(super) fn build(matches: &ArgMatches) -> Result<Report, Refusal> {
    let session = required(matches, "SESSION")?;
    let lane = parse::<wire::CanonicalUuid>(matches, "lane")?;
    let vendor = parse::<wire::Name>(matches, "vendor")?;
    let stream = required(matches, "stream")?;
    let source = parse::<wire::CanonicalUuid>(matches, "source")?;
    let member = parse::<u64>(matches, "member")?;
    let offset = parse::<u64>(matches, "offset")?;
    if stream != wire::body::TASK_CREATION_CONTEXT_STREAM {
        return Err(refusal(
            "session::context-stream-unregistered",
            Some(stream),
            "use the selected accounted stream retained by task creation context",
        ));
    }

    let current = std::env::current_dir().map_err(|error| {
        refusal(
            "session::context-repository-unavailable",
            Some(error.to_string()),
            "run the retained command from the task repository",
        )
    })?;
    let repository = Repository::discover(&current).map_err(|error| {
        refusal(
            "session::context-repository-unavailable",
            Some(error.to_string()),
            "run the retained command from the task repository",
        )
    })?;
    let state = repository.admit_state().map_err(|error| {
        refusal(
            "session::context-repository-unavailable",
            Some(error.to_string()),
            "restore the repository-owned Spice state before rehydrating task context",
        )
    })?;
    let root = state.tape().join(lane.to_string()).join(vendor.as_str());
    let tape = retained_tape(&root, source)?;
    let (extents, boundary_index) = retained_boundary(&tape, member, offset)?;

    let before = records_before(&tape, &extents, boundary_index, offset)?;
    let after = records_after(&tape, &extents, boundary_index, offset)?;
    Ok(Report::Context {
        identity: session,
        lane: lane.to_string(),
        vendor: vendor.to_string(),
        stream,
        source: source.to_string(),
        member,
        offset,
        before,
        after,
    })
}

fn retained_tape(
    root: &std::path::Path,
    source: wire::CanonicalUuid,
) -> Result<tape::Tape, Refusal> {
    let tape = tape::Tape::open_existing(root).map_err(|error| {
        refusal(
            "session::context-source-missing",
            Some(format!("{}: {error}", root.display())),
            "restore the exact repository-owned lane/vendor tape",
        )
    })?;
    let observed = tape.source_id().map_err(|error| {
        refusal(
            "session::context-source-missing",
            Some(error.to_string()),
            "restore the tape source identity retained by the task",
        )
    })?;
    if observed.as_str() != source.to_string() {
        return Err(refusal(
            "session::context-source-replaced",
            Some(format!("expected={source} observed={observed}")),
            "restore the exact tape generation retained by the task",
        ));
    }
    Ok(tape)
}

fn retained_boundary(
    tape: &tape::Tape,
    member: u64,
    offset: u64,
) -> Result<(Vec<tape::Extent>, usize), Refusal> {
    let extents = tape.extents().map_err(|error| {
        refusal(
            "session::context-source-unreadable",
            Some(error.to_string()),
            "repair or restore the retained tape generation",
        )
    })?;
    let boundary_index = extents
        .iter()
        .position(|extent| extent.member == member)
        .ok_or_else(|| {
            refusal(
                "session::context-source-truncated",
                Some(format!("member={member} offset={offset}")),
                "restore the tape member carrying the retained boundary",
            )
        })?;
    let extent = extents[boundary_index];
    if offset > extent.length {
        return Err(refusal(
            "session::context-source-truncated",
            Some(format!(
                "member={member} offset={offset} length={}",
                extent.length
            )),
            "restore the complete tape bytes through the retained boundary",
        ));
    }
    if !matches!(
        preceding_byte(tape, &extents, boundary_index, offset)?,
        None | Some(b'\n')
    ) {
        return Err(refusal(
            "session::context-non-boundary",
            Some(format!("member={member} offset={offset}")),
            "use the next complete-record member and offset retained by task authority",
        ));
    }

    Ok((extents, boundary_index))
}

fn preceding_byte(
    tape: &tape::Tape,
    extents: &[tape::Extent],
    index: usize,
    offset: u64,
) -> Result<Option<u8>, Refusal> {
    let position = if offset > 0 {
        Some((extents[index].member, offset - 1))
    } else {
        extents[..index]
            .iter()
            .rfind(|extent| extent.length > 0)
            .map(|extent| (extent.member, extent.length - 1))
    };
    let Some((member, offset)) = position else {
        return Ok(None);
    };
    let bytes = tape
        .read(tape::Locus::new(member, offset, 1))
        .map_err(|error| source_read_refusal(member, offset, error))?;
    Ok(bytes.first().copied())
}

fn records_before(
    tape: &tape::Tape,
    extents: &[tape::Extent],
    boundary_index: usize,
    offset: u64,
) -> Result<Vec<String>, Refusal> {
    let mut chunks = Vec::new();
    let mut start = None;
    let mut remaining = WINDOW_BYTES;
    for index in (0..=boundary_index).rev() {
        if remaining == 0 {
            break;
        }
        let extent = extents[index];
        let end = if index == boundary_index {
            offset
        } else {
            extent.length
        };
        let length = end.min(remaining);
        if length == 0 {
            continue;
        }
        let offset = end - length;
        let bytes = tape
            .read(tape::Locus::new(extent.member, offset, length))
            .map_err(|error| source_read_refusal(extent.member, offset, error))?;
        chunks.push(bytes);
        start = Some((index, offset));
        remaining -= length;
    }
    chunks.reverse();
    let bytes = chunks.concat();
    let starts_at_boundary = match start {
        Some((index, offset)) => matches!(
            preceding_byte(tape, extents, index, offset)?,
            None | Some(b'\n')
        ),
        None => true,
    };
    let mut records = complete_records(&bytes, starts_at_boundary, true)?;
    if records.len() > WINDOW_RECORDS {
        records.drain(..records.len() - WINDOW_RECORDS);
    }
    Ok(records)
}

fn records_after(
    tape: &tape::Tape,
    extents: &[tape::Extent],
    boundary_index: usize,
    offset: u64,
) -> Result<Vec<String>, Refusal> {
    let mut bytes = Vec::new();
    let mut remaining = WINDOW_BYTES;
    for (index, extent) in extents.iter().copied().enumerate().skip(boundary_index) {
        if remaining == 0 {
            break;
        }
        let start = if index == boundary_index { offset } else { 0 };
        let length = extent.length.saturating_sub(start).min(remaining);
        if length == 0 {
            continue;
        }
        let chunk = tape
            .read(tape::Locus::new(extent.member, start, length))
            .map_err(|error| source_read_refusal(extent.member, start, error))?;
        bytes.extend(chunk);
        remaining -= length;
    }
    let mut records = complete_records(&bytes, true, false)?;
    records.truncate(WINDOW_RECORDS);
    Ok(records)
}

fn complete_records(
    bytes: &[u8],
    starts_at_boundary: bool,
    ends_at_boundary: bool,
) -> Result<Vec<String>, Refusal> {
    let start = if starts_at_boundary {
        0
    } else {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |position| position + 1)
    };
    let end = if ends_at_boundary {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(start, |position| position + 1)
    };
    let text = std::str::from_utf8(&bytes[start..end]).map_err(|error| {
        refusal(
            "session::context-source-undecodable",
            Some(error.to_string()),
            "restore the original UTF-8 vendor transcript bytes",
        )
    })?;
    Ok(text
        .split_inclusive('\n')
        .map(|row| row.trim_end_matches('\n').to_owned())
        .collect())
}

fn required(matches: &ArgMatches, name: &str) -> Result<String, Refusal> {
    matches.get_one::<String>(name).cloned().ok_or_else(|| {
        refusal(
            "session::context-coordinate-missing",
            Some(name.to_owned()),
            "run the complete command rendered by task show or task next",
        )
    })
}

fn parse<T>(matches: &ArgMatches, name: &str) -> Result<T, Refusal>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = required(matches, name)?;
    value.parse().map_err(|error| {
        refusal(
            "session::context-coordinate-invalid",
            Some(format!("{name}={value}: {error}")),
            "run the unmodified command rendered by task show or task next",
        )
    })
}

fn source_read_refusal(member: u64, offset: u64, error: tape::Error) -> Refusal {
    refusal(
        "session::context-source-truncated",
        Some(format!("member={member} offset={offset}: {error}")),
        "restore the exact retained tape generation and member bytes",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-session-context-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).expect("context scratch root is unique");
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("context scratch is removed");
        }
    }

    fn source(tape: &tape::Tape) -> wire::CanonicalUuid {
        tape.source_id()
            .expect("fixture source reads")
            .as_str()
            .parse()
            .expect("tape source is canonical")
    }

    #[test]
    fn retained_context_reads_a_bounded_complete_record_window_from_a_real_tape() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        let complete = (0..21)
            .map(|index| format!("record-{index:02}\n"))
            .collect::<String>();
        let boundary = (0..11)
            .map(|index| format!("record-{index:02}\n").len() as u64)
            .sum::<u64>();
        tape.tee(format!("{complete}partial-tail").as_bytes())
            .expect("fixture transcript appends");

        let admitted = retained_tape(&scratch.0, source(&tape)).expect("exact source admits");
        let (extents, index) =
            retained_boundary(&admitted, 1, boundary).expect("complete boundary admits");
        assert_eq!(
            records_before(&admitted, &extents, index, boundary).expect("before window reads"),
            (3..11)
                .map(|index| format!("record-{index:02}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            records_after(&admitted, &extents, index, boundary).expect("after window reads"),
            (11..19)
                .map(|index| format!("record-{index:02}"))
                .collect::<Vec<_>>(),
            "the bounded window omits both excess complete records and an unterminated tail"
        );
    }

    #[test]
    fn retained_context_refuses_missing_replaced_truncated_and_non_boundary_sources() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        tape.tee(b"first\nsecond\n")
            .expect("fixture transcript appends");
        let actual = source(&tape);
        let absent = scratch.0.join("absent");
        let expected = [
            "018f82d4-6f59-7d06-92e3-bce9131163c2",
            "018f82d4-6f59-7d06-92e3-bce9131163c3",
        ]
        .into_iter()
        .map(|value| value.parse().expect("fixture UUID"))
        .find(|candidate| candidate != &actual)
        .expect("one fixture UUID differs from the tape source");

        assert_eq!(
            retained_tape(&absent, actual)
                .expect_err("missing tape refuses")
                .reason(),
            "session::context-source-missing"
        );
        assert_eq!(
            retained_tape(&scratch.0, expected)
                .expect_err("another source generation refuses")
                .reason(),
            "session::context-source-replaced"
        );
        assert_eq!(
            retained_boundary(&tape, 2, 0)
                .expect_err("missing member refuses")
                .reason(),
            "session::context-source-truncated"
        );
        assert_eq!(
            retained_boundary(&tape, 1, 99)
                .expect_err("missing bytes refuse")
                .reason(),
            "session::context-source-truncated"
        );
        assert_eq!(
            retained_boundary(&tape, 1, 2)
                .expect_err("mid-record coordinate refuses")
                .reason(),
            "session::context-non-boundary"
        );
    }

    #[test]
    fn retained_context_joins_records_across_members_and_checks_the_member_edge() {
        let scratch = Scratch::new();
        let tape = tape::Tape::open(&scratch.0).expect("fixture tape opens");
        tape.tee(b"joined-").expect("record prefix appends");
        tape.seal().expect("record prefix seals");
        tape.tee(b"record\nnext\n").expect("record suffix appends");

        assert_eq!(
            retained_boundary(&tape, 2, 0)
                .expect_err("a member split is not a record boundary")
                .reason(),
            "session::context-non-boundary"
        );
        let (extents, index) =
            retained_boundary(&tape, 2, 7).expect("complete cross-member record admits");
        assert_eq!(
            records_before(&tape, &extents, index, 7).expect("cross-member window reads"),
            ["joined-record"]
        );
        assert_eq!(
            records_after(&tape, &extents, index, 7).expect("following window reads"),
            ["next"]
        );
    }

    #[test]
    fn record_decoding_discards_clipped_codepoints_but_refuses_invalid_complete_records() {
        assert_eq!(
            complete_records(&[0x82, 0xac, b'\n', b'o', b'k', b'\n'], false, true)
                .expect("clipped UTF-8 belongs to the discarded partial record"),
            ["ok"]
        );
        assert_eq!(
            complete_records(&[0xff, b'\n'], true, true)
                .expect_err("invalid complete record refuses")
                .reason(),
            "session::context-source-undecodable"
        );
    }
}
