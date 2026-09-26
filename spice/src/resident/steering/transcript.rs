//! Durable transcript record decoding for acknowledgment evidence.

use super::*;
use std::time::{Duration, Instant as Clock};

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

/// One recent window, shared by every pending key in a lane's recovery sweep.
/// Work limits include boundary discovery, decoding, and evidence matching.
const SEARCH_WINDOW_BYTES: u64 = 1024 * 1024;
const SEARCH_SOURCE_BYTES: u64 = 2 * 1024 * 1024;
const SEARCH_RECORDS: usize = 4096;
const SEARCH_PASSES: usize = 256;
const SEARCH_DURATION: Duration = Duration::from_secs(1);

pub(super) struct TranscriptEvidence {
    tail: listen::TailScan,
    source: CanonicalUuid,
    deadline: Clock,
    repair: &'static str,
}

impl TranscriptEvidence {
    pub(super) fn read(
        paths: &RuntimePaths,
        lane: CanonicalUuid,
        vendor: &wire::Name,
        token: &str,
    ) -> Result<Self, Outcome<CommandAnswer>> {
        let repair = header_repair(vendor);
        let deadline = Clock::now() + SEARCH_DURATION;
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
        let registry = spice::extension::registry().map_err(|error| {
            typed_protocol_fault("steer::extension-registry-invalid", &error.to_string())
        })?;
        let dialect = registry
            .vendor(vendor.as_str())
            .ok_or_else(|| {
                typed_protocol_fault("steer::transcript-vendor-invalid", vendor.as_str())
            })?
            .dialect();
        let budget = listen::TailBudget::new(
            SEARCH_WINDOW_BYTES,
            SEARCH_SOURCE_BYTES,
            SEARCH_RECORDS,
            SEARCH_PASSES,
            SEARCH_DURATION,
        )
        .expect("steering evidence allowances are positive");
        let tail = listen::scan_tail(&path, dialect, budget).map_err(|error| match error {
            listen::ScanError::WorkLimit { .. }
            | listen::ScanError::Tape {
                source: tape::Error::Refused(tape::Refusal::MemberReadLimit { .. }),
                ..
            } => search_limited(token, &error.to_string(), repair),
            _ => typed_store_fault("steer::transcript-tape-unreadable", &error.to_string()),
        })?;
        let source = tail
            .scan
            .checkpoint
            .source
            .as_str()
            .parse::<CanonicalUuid>()
            .map_err(|error| {
                typed_protocol_fault("steer::transcript-source-invalid", &error.to_string())
            })?;
        Ok(Self {
            tail,
            source,
            deadline,
            repair,
        })
    }

    pub(super) fn exhausted(&self) -> bool {
        Clock::now() >= self.deadline
    }

    /// Search newest records and events first; a later citation does not erase
    /// an older placed answer within the admitted window.
    pub(super) fn recognize(
        &self,
        token: &str,
        disposition: Option<Disposition>,
        standing: &impl steer::Standing,
    ) -> Result<TranscriptRecognition, Outcome<CommandAnswer>> {
        let mut newest = NewestMessage::default();
        let mut latest_miss = None;
        for record in self.tail.scan.records.iter().rev() {
            if self.exhausted() {
                return Err(search_limited(
                    token,
                    "the evidence search reached its time limit",
                    self.repair,
                ));
            }
            if let Some(recognition) =
                scan_record(record, token, disposition, standing, &mut newest)?
            {
                let recognition = TranscriptRecognition {
                    locus: TranscriptLocus {
                        source: Some(self.source),
                        member: record.locus.member,
                        offset: record.locus.offset,
                        length: record.locus.length,
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
        if self.tail.start.next > listen::Position::start() {
            return Err(search_limited(
                token,
                "no placed answer in the bounded recent transcript window",
                self.repair,
            ));
        }
        latest_miss.ok_or_else(|| newest.evidence_absent(token, self.repair))
    }
}

fn search_limited(token: &str, detail: &str, repair: &str) -> Outcome<CommandAnswer> {
    refusal(
        "steer::transcript-search-limited",
        Some("a placed answer within the bounded recent transcript".to_owned()),
        Some(token.to_owned()),
        &format!("{detail}; the directive remains pending; {repair}"),
    )
}

/// Search a bounded recent transcript window, newest evidence first.
pub(super) fn transcript_recognition(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    vendor: &wire::Name,
    token: &str,
    disposition: Option<Disposition>,
    standing: &impl steer::Standing,
) -> Result<TranscriptRecognition, Outcome<CommandAnswer>> {
    TranscriptEvidence::read(paths, lane, vendor, token)?.recognize(token, disposition, standing)
}

struct RecordRecognition {
    event: u32,
    line: u32,
    column: u32,
    outcome: ScanOutcome,
}

fn scan_record(
    record: &listen::Record,
    token: &str,
    disposition: Option<Disposition>,
    standing: &impl steer::Standing,
    newest: &mut NewestMessage,
) -> Result<Option<RecordRecognition>, Outcome<CommandAnswer>> {
    let listen::RecordPayload::Events(events) = &record.payload else {
        return Ok(None);
    };
    let mut latest_miss = None;
    for (event_index, sourced) in events.iter().enumerate().rev() {
        let event = &sourced.event;
        newest.observe(event, record.locus);
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
    Ok(latest_miss)
}

fn checked_locus_part(part: usize) -> Result<u32, Outcome<CommandAnswer>> {
    u32::try_from(part).map_err(|error| {
        typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
    })
}
