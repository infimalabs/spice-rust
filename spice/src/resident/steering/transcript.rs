//! Durable transcript record decoding for acknowledgment evidence.

use super::*;

#[cfg(test)]
mod tests;

pub(super) struct TranscriptRecognition {
    pub(super) locus: TranscriptLocus,
    pub(super) outcome: ScanOutcome,
}

/// Repair when the lane wrote no leading header as durable assistant text.
const COMMAND_HEADER_REPAIR: &str = "emit an ACK or NACK header as the first line of visible assistant text, ensure that line \
     is durably present on the transcript, then invoke ack";

/// OMP retires interjected steering from prose itself, without an ack command.
const INTERJECTED_HEADER_REPAIR: &str = "send the ACK or NACK header as the first line of visible assistant text and let that \
     message land; the durable assistant text must name the key and disposition";

fn header_repair(vendor: &wire::Name) -> &'static str {
    if vendor.as_str() == "omp" {
        INTERJECTED_HEADER_REPAIR
    } else {
        COMMAND_HEADER_REPAIR
    }
}

/// Characters of the newest assistant text the refusal quotes back.
const QUOTED_LINE: usize = 80;

/// The shape of the newest assistant message on the tape.
///
/// The message is every assistant record newer than the newest operator
/// prose or tool result. It is read newest record first while the scan finds
/// no assistant prose recognizing the token, so the refusal names the record
/// the lane actually wrote instead of the header it did not.
#[derive(Default)]
struct NewestMessage {
    text: Option<(tape::Locus, String)>,
    reasoning: Option<tape::Locus>,
    tool: Option<tape::Locus>,
    bound: Option<tape::Locus>,
}

impl NewestMessage {
    fn observe(&mut self, event: &listen::Event, locus: tape::Locus) {
        if self.bound.is_some() {
            return;
        }
        match event {
            listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
                self.text
                    .get_or_insert_with(|| (locus, quoted_line(&prose.text)));
            }
            listen::Event::Prose(_) => self.bound = Some(locus),
            listen::Event::Tool(tool) => match tool.moment {
                listen::ToolMoment::Result(_) => self.bound = Some(locus),
                listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => {
                    self.tool.get_or_insert(locus);
                }
            },
            listen::Event::Reasoning(_) => {
                self.reasoning.get_or_insert(locus);
            }
            listen::Event::Session(_)
            | listen::Event::Usage(_)
            | listen::Event::Compaction(_)
            | listen::Event::Failure(_) => {}
        }
    }

    fn evidence_absent(self, token: &str, repair: &str) -> Outcome<CommandAnswer> {
        if let Some((locus, line)) = self.text {
            return evidence_refusal(
                token,
                &format!(
                    "the newest assistant text (member {} offset {}) does not carry the header: \"{line}\"",
                    locus.member, locus.offset
                ),
                repair,
            );
        }
        if let Some(locus) = self.reasoning {
            return evidence_refusal(
                token,
                &format!(
                    "the newest assistant message (member {} offset {}) persisted as reasoning with no text record",
                    locus.member, locus.offset
                ),
                repair,
            );
        }
        if let Some(locus) = self.tool {
            return evidence_refusal(
                token,
                &format!(
                    "the newest assistant message (member {} offset {}) is a tool call with no text record",
                    locus.member, locus.offset
                ),
                repair,
            );
        }
        if let Some(locus) = self.bound {
            return evidence_refusal(
                token,
                &format!(
                    "no assistant text newer than the operator record (member {} offset {})",
                    locus.member, locus.offset
                ),
                repair,
            );
        }
        evidence_refusal(token, "no assistant text on the tape", repair)
    }
}

/// The first line of `text`, cut to [`QUOTED_LINE`] characters.
fn quoted_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut quoted: String = line.chars().take(QUOTED_LINE).collect();
    if quoted.len() < line.len() {
        quoted.push_str("...");
    }
    quoted
}

/// Find the latest placed assistant answer to `token`, newest record first.
///
/// A later quotation or buried mention cannot erase an answer already spoken.
/// If no placed answer exists, preserve the newest miss for the refusal.
///
/// A requested `disposition` must match the marker the transcript carries;
/// `None` accepts whichever terminal marker the transcript shows, which is
/// how the exit sweep reads an answer nobody has yet claimed on the command
/// line.
pub(super) fn transcript_recognition(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    vendor: &wire::Name,
    token: &str,
    disposition: Option<Disposition>,
    standing: &impl steer::Standing,
) -> Result<TranscriptRecognition, Outcome<CommandAnswer>> {
    let repair = header_repair(vendor);
    let path = paths.tape.join(lane.to_string()).join(vendor.as_str());
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(typed_store_fault(
                "steer::transcript-tape-invalid",
                &format!("{} is not a tape directory", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(evidence_refusal(
                token,
                "the lane has no durable transcript tape",
                repair,
            ));
        }
        Err(error) => {
            return Err(typed_store_fault(
                "steer::transcript-tape-unreadable",
                &error.to_string(),
            ));
        }
    }
    let tape = tape::Tape::open_existing(&path).map_err(|error| {
        typed_store_fault("steer::transcript-tape-unreadable", &error.to_string())
    })?;
    let source = tape
        .source_id()
        .map_err(|error| {
            typed_store_fault("steer::transcript-tape-unreadable", &error.to_string())
        })?
        .as_str()
        .parse::<CanonicalUuid>()
        .map_err(|error| {
            typed_protocol_fault("steer::transcript-source-invalid", &error.to_string())
        })?;
    let extents = tape.extents().map_err(|error| {
        typed_store_fault("steer::transcript-tape-unreadable", &error.to_string())
    })?;
    let mut newest = NewestMessage::default();
    let mut latest_miss = None;
    for extent in extents.into_iter().rev() {
        let whole = tape::Locus::new(extent.member, 0, extent.length);
        let bytes = tape.read(whole).map_err(|error| {
            typed_store_fault("steer::transcript-tape-unreadable", &error.to_string())
        })?;
        for (start, end) in record_ranges(&bytes).into_iter().rev() {
            let offset = u64::try_from(start).map_err(|error| {
                typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
            })?;
            let length = u64::try_from(end - start).map_err(|error| {
                typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
            })?;
            let record_locus = tape::Locus::new(extent.member, offset, length);
            if let Some(recognition) = scan_record(
                &bytes[start..end],
                record_locus,
                token,
                disposition,
                standing,
                &mut newest,
            )? {
                let recognition = TranscriptRecognition {
                    locus: TranscriptLocus {
                        source: Some(source),
                        member: extent.member,
                        offset,
                        length,
                        event: recognition.event,
                        line: recognition.line,
                        column: recognition.column,
                    },
                    outcome: recognition.outcome,
                };
                if matches!(recognition.outcome, ScanOutcome::Retired { .. }) {
                    return Ok(recognition);
                }
                latest_miss.get_or_insert(recognition);
            }
        }
    }
    latest_miss.ok_or_else(|| newest.evidence_absent(token, repair))
}

struct RecordRecognition {
    event: u32,
    line: u32,
    column: u32,
    outcome: ScanOutcome,
}

fn scan_record(
    bytes: &[u8],
    locus: tape::Locus,
    token: &str,
    disposition: Option<Disposition>,
    standing: &impl steer::Standing,
    newest: &mut NewestMessage,
) -> Result<Option<RecordRecognition>, Outcome<CommandAnswer>> {
    let registry = spice::extension::registry().map_err(|error| {
        typed_protocol_fault("steer::extension-registry-invalid", &error.to_string())
    })?;
    let mut latest_miss = None;
    for vendor in registry.vendors() {
        let decoded = vendor.decode(bytes, locus).map_err(|error| {
            typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
        })?;
        for (event_index, event) in decoded.events().iter().enumerate().rev() {
            newest.observe(event, locus);
            let listen::Event::Prose(prose) = event else {
                continue;
            };
            // Only the occupant answers for the lane. Operator input reaches
            // the same tape as user records, and a continuation summary or a
            // restated inbox spells the header without anyone having said it.
            if prose.role != listen::ProseRole::Assistant {
                continue;
            }
            let Some(recognition) = steer::decisive(&prose.text, token, standing) else {
                continue;
            };
            if let ScanOutcome::Retired {
                disposition: observed,
                ..
            } = &recognition.outcome
                && let Some(requested) = disposition
                && *observed != requested
            {
                return Err(refusal(
                    "steer::transcript-disposition-mismatch",
                    Some(format!("{requested:?}").to_lowercase()),
                    Some(format!("{observed:?}").to_lowercase()),
                    "make the ack command disposition match the latest transcript marker",
                ));
            }
            let recognition = RecordRecognition {
                event: checked_locus_part(event_index)?,
                line: checked_locus_part(recognition.locus.line)?,
                column: checked_locus_part(recognition.locus.column)?,
                outcome: recognition.outcome,
            };
            if matches!(recognition.outcome, ScanOutcome::Retired { .. }) {
                return Ok(Some(recognition));
            }
            latest_miss.get_or_insert(recognition);
        }
    }
    Ok(latest_miss)
}

fn checked_locus_part(part: usize) -> Result<u32, Outcome<CommandAnswer>> {
    u32::try_from(part).map_err(|error| {
        typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
    })
}

fn record_ranges(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            ranges.push((start, index + 1));
            start = index + 1;
        }
    }
    if start < bytes.len() {
        ranges.push((start, bytes.len()));
    }
    ranges
}
