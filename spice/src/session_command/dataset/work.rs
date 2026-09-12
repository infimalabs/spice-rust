//! Fixed work accounting for one session transcript reduction.

use std::time::{Duration, Instant};

use listen::ReasoningContent;

use super::*;

const REPORT_SOURCE_BYTES: u64 = 512 * 1_024 * 1_024;
const REPORT_RECORDS: u64 = 256 * 1_024;
const REPORT_EVENTS: u64 = 1_024 * 1_024;
const REPORT_RETAINED_TEXT_BYTES: u64 = 512 * 1_024 * 1_024;
const REPORT_PASSES: u32 = 64;
const REPORT_DURATION: Duration = Duration::from_secs(30);
const MAX_RESIDUES: usize = 256;
const MAX_RESIDUE_BYTES: u64 = 256 * 1_024;
const RESIDUE_SUMMARY_RESERVE: u64 = 512;

pub(super) fn decoded_row_work(row: &DecodedRow) -> (usize, u64) {
    match row {
        DecodedRow::Typed(record) => (record.events.len(), typed_record_text_bytes(record)),
        DecodedRow::Residue(residue) => (0, residue_bytes(residue)),
        DecodedRow::Ignored => (0, 0),
    }
}

fn typed_record_text_bytes(record: &TypedRecord) -> u64 {
    let context = context_text_bytes(&record.context);
    let events = record
        .events
        .iter()
        .map(|event| {
            context_text_bytes(&event.context).saturating_add(event_text_bytes(&event.event))
        })
        .fold(0, u64::saturating_add);
    usize_bytes(record.source.len())
        .saturating_add(context)
        .saturating_add(events)
}

fn context_text_bytes(context: &RecordContext) -> u64 {
    context
        .timestamp
        .iter()
        .chain(context.turn_id.iter())
        .map(|value| usize_bytes(value.len()))
        .fold(0, u64::saturating_add)
}

fn event_text_bytes(event: &Event) -> u64 {
    match event {
        Event::Session(session) => usize_bytes(session.identity.len()),
        Event::Prose(prose) => usize_bytes(prose.text.len()).saturating_add(
            prose
                .phase
                .as_ref()
                .map_or(0, |phase| usize_bytes(phase.len())),
        ),
        Event::Reasoning(reasoning) => match &reasoning.content {
            ReasoningContent::Plain(rows) => rows
                .iter()
                .map(|row| usize_bytes(row.len()))
                .fold(0, u64::saturating_add),
            ReasoningContent::Opaque => 0,
        },
        Event::Tool(tool) => usize_bytes(tool.call_id.len()).saturating_add(match &tool.moment {
            ToolMoment::Call { name, arguments } | ToolMoment::Update { name, arguments } => {
                usize_bytes(name.len()).saturating_add(usize_bytes(arguments.len()))
            }
            ToolMoment::Result(outcome) => outcome
                .output
                .as_ref()
                .map_or(0, |output| usize_bytes(output.len())),
        }),
        Event::Usage(_) => 0,
        Event::Compaction(compaction) => compaction
            .summary
            .as_ref()
            .map_or(0, |summary| usize_bytes(summary.len())),
        Event::Failure(failure) => usize_bytes(failure.message.len()),
    }
}

fn usize_bytes(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy)]
pub(super) struct SessionWorkBudget {
    source_bytes: u64,
    records: u64,
    events: u64,
    retained_text_bytes: u64,
    passes: u32,
    duration: Duration,
}

impl SessionWorkBudget {
    pub(super) fn report() -> Self {
        Self::new(
            REPORT_SOURCE_BYTES,
            REPORT_RECORDS,
            REPORT_EVENTS,
            REPORT_RETAINED_TEXT_BYTES,
            REPORT_PASSES,
            REPORT_DURATION,
        )
        .expect("repository session report budget is positive")
    }

    pub(super) fn new(
        source_bytes: u64,
        records: u64,
        events: u64,
        retained_text_bytes: u64,
        passes: u32,
        duration: Duration,
    ) -> Result<Self, Refusal> {
        if source_bytes == 0
            || records == 0
            || events == 0
            || retained_text_bytes == 0
            || passes == 0
            || duration.is_zero()
        {
            return Err(refusal(
                "session::transcript-work-budget-invalid",
                Some(format!(
                    "source_bytes={source_bytes} records={records} events={events} retained_text_bytes={retained_text_bytes} passes={passes} duration={duration:?}"
                )),
                "use one positive repository-owned session work allowance",
            ));
        }
        Ok(Self {
            source_bytes,
            records,
            events,
            retained_text_bytes,
            passes,
            duration,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SessionWorkReport {
    pub(super) source_bytes: u64,
    pub(super) records: u64,
    pub(super) events: u64,
    pub(super) retained_text_bytes: u64,
    pub(super) passes: u32,
    pub(super) elapsed: Duration,
}

pub(super) struct SessionWork {
    budget: SessionWorkBudget,
    report: SessionWorkReport,
    started: Instant,
}

impl SessionWork {
    pub(super) fn new(budget: SessionWorkBudget) -> Self {
        Self {
            budget,
            report: SessionWorkReport::default(),
            started: Instant::now(),
        }
    }

    pub(super) fn begin_pass(&mut self, at: &str) -> Result<(), Refusal> {
        self.check_deadline(at)?;
        let next = self.report.passes.saturating_add(1);
        if next > self.budget.passes {
            return Err(self.exhausted("passes", at));
        }
        self.report.passes = next;
        Ok(())
    }

    pub(super) fn admit_source_bytes(&mut self, bytes: u64, at: &str) -> Result<(), Refusal> {
        self.check_deadline(at)?;
        let next = self.report.source_bytes.checked_add(bytes);
        if next.is_none_or(|next| next > self.budget.source_bytes) {
            return Err(self.exhausted("source-bytes", at));
        }
        self.report.source_bytes = next.expect("admitted source-byte sum exists");
        Ok(())
    }

    pub(super) fn admit_record(&mut self, at: &str) -> Result<(), Refusal> {
        self.ensure_record(at)?;
        self.report.records = self.report.records.saturating_add(1);
        Ok(())
    }

    pub(super) fn ensure_record(&mut self, at: &str) -> Result<(), Refusal> {
        self.check_deadline(at)?;
        if self.report.records >= self.budget.records {
            Err(self.exhausted("records", at))
        } else {
            Ok(())
        }
    }

    pub(super) fn admit_decoded(
        &mut self,
        events: usize,
        retained_text_bytes: u64,
        at: &str,
    ) -> Result<(), Refusal> {
        self.check_deadline(at)?;
        let events = u64::try_from(events).unwrap_or(u64::MAX);
        let next_events = self.report.events.checked_add(events);
        if next_events.is_none_or(|next| next > self.budget.events) {
            return Err(self.exhausted("events", at));
        }
        let next_text = self
            .report
            .retained_text_bytes
            .checked_add(retained_text_bytes);
        if next_text.is_none_or(|next| next > self.budget.retained_text_bytes) {
            return Err(self.exhausted("retained-text-bytes", at));
        }
        self.report.events = next_events.expect("admitted event sum exists");
        self.report.retained_text_bytes = next_text.expect("admitted text-byte sum exists");
        Ok(())
    }

    pub(super) fn check_deadline(&mut self, at: &str) -> Result<(), Refusal> {
        self.report.elapsed = self.started.elapsed();
        if self.report.elapsed > self.budget.duration {
            Err(self.exhausted("wall-duration", at))
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub(super) fn report(&mut self) -> SessionWorkReport {
        self.report.elapsed = self.started.elapsed();
        self.report
    }

    fn exhausted(&self, limit: &str, at: &str) -> Refusal {
        refusal(
            "session::transcript-work-exhausted",
            Some(format!(
                "limit={limit} at={at:?} work=source_bytes:{} records:{} events:{} retained_text_bytes:{} passes:{} elapsed:{:?}",
                self.report.source_bytes,
                self.report.records,
                self.report.events,
                self.report.retained_text_bytes,
                self.report.passes,
                self.report.elapsed,
            )),
            "narrow the immutable transcript inputs or filters and retry; the observed source boundary identifies the exact exhausted position",
        )
    }
}

pub(super) struct ResidueLedger {
    rows: Vec<Residue>,
    bytes: u64,
    omitted: u64,
    omitted_bytes: u64,
}

impl ResidueLedger {
    pub(super) fn new() -> Self {
        Self {
            rows: Vec::new(),
            bytes: 0,
            omitted: 0,
            omitted_bytes: 0,
        }
    }

    pub(super) fn push(&mut self, residue: Residue) {
        let bytes = residue_bytes(&residue);
        let fits = self.rows.len() < MAX_RESIDUES.saturating_sub(1)
            && self
                .bytes
                .checked_add(bytes)
                .and_then(|bytes| bytes.checked_add(RESIDUE_SUMMARY_RESERVE))
                .is_some_and(|bytes| bytes <= MAX_RESIDUE_BYTES);
        if fits {
            self.bytes = self.bytes.saturating_add(bytes);
            self.rows.push(residue);
        } else {
            self.omitted = self.omitted.saturating_add(1);
            self.omitted_bytes = self.omitted_bytes.saturating_add(bytes);
        }
    }

    pub(super) fn finish(mut self) -> Vec<Residue> {
        if self.omitted > 0 {
            self.rows.push(Residue {
                source: String::new(),
                line: 0,
                kind: "residue-overflow".to_owned(),
                detail: format!(
                    "{} transcript residues totaling {} logical text bytes were omitted",
                    self.omitted, self.omitted_bytes
                ),
                recovery: "narrow the transcript inputs to inspect omitted residue evidence"
                    .to_owned(),
            });
        }
        self.rows
    }
}

fn residue_bytes(residue: &Residue) -> u64 {
    [
        &residue.source,
        &residue.kind,
        &residue.detail,
        &residue.recovery,
    ]
    .into_iter()
    .map(|value| u64::try_from(value.len()).unwrap_or(u64::MAX))
    .fold(0, u64::saturating_add)
}
