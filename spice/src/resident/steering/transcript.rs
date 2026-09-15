//! Durable transcript record decoding for acknowledgment evidence.

use super::*;

pub(super) struct TranscriptRecognition {
    pub(super) locus: TranscriptLocus,
    pub(super) outcome: ScanOutcome,
}

/// Find the latest assistant prose recognizing `token`, newest record first.
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
            )? {
                return Ok(TranscriptRecognition {
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
                });
            }
        }
    }
    Err(evidence_refusal(
        token,
        "no assistant transcript record recognizes this token",
    ))
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
) -> Result<Option<RecordRecognition>, Outcome<CommandAnswer>> {
    let registry = spice::extension::registry().map_err(|error| {
        typed_protocol_fault("steer::extension-registry-invalid", &error.to_string())
    })?;
    for vendor in registry.vendors() {
        let decoded = vendor.decode(bytes, locus).map_err(|error| {
            typed_protocol_fault("steer::transcript-locus-invalid", &error.to_string())
        })?;
        for (event_index, event) in decoded.events().iter().enumerate().rev() {
            let listen::Event::Prose(prose) = event else {
                continue;
            };
            for recognition in steer::scan(&prose.text, standing).into_iter().rev() {
                if recognition_token(&recognition.outcome) != token {
                    continue;
                }
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
                return Ok(Some(RecordRecognition {
                    event: checked_locus_part(event_index)?,
                    line: checked_locus_part(recognition.locus.line)?,
                    column: checked_locus_part(recognition.locus.column)?,
                    outcome: recognition.outcome,
                }));
            }
        }
    }
    Ok(None)
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

fn recognition_token(outcome: &ScanOutcome) -> &str {
    match outcome {
        ScanOutcome::Retired { key, .. } => key.as_str(),
        ScanOutcome::Missed { token, .. } => token,
    }
}
